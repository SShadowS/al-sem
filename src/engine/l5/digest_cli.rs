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
    ConeIndexes, DEFAULT_MAX_DEPTH, DEFAULT_MAX_VISITED, UnresolvedConeItem, UnresolvedTraversal,
    unresolved_cone_in,
};
use crate::engine::line_base::shown_opt;
use crate::engine::session::{Cell, RunKey};
use crate::program::model::workspace::{Model, ModelRoutine};

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
/// impact`'s accounting reads paths exactly as the changed roots are read,
/// anchors included (#363).
pub(crate) fn path_keys(root: &PathRoot, path: &str) -> Vec<String> {
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
    /// An existing path with no `.al` entry → treat as --diff.
    Diff(String),
    /// Comma-list with any `.al` entry → treat as --changed-files, whether or
    /// not the path exists (#362).
    Files(Vec<String>),
    /// Else → treat as --changed-routines.
    Routines(Vec<String>),
}

/// `autoDetectChanged` (cli/digest.ts). A `.al` entry makes the value file inputs
/// before existence is asked: a `.al` path is AL source wherever the command
/// runs, never diff text (#362). Else an existing path is a diff file, and else a
/// routine selector (graceful miscategorization, NOT an OS error). `exists` is
/// injected so this stays a pure, testable function.
pub fn auto_detect_changed_with(value: &str, exists: impl Fn(&str) -> bool) -> ChangedAutoDetect {
    let parts: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.iter().any(|p| p.to_lowercase().ends_with(".al")) {
        ChangedAutoDetect::Files(parts)
    } else if exists(value) {
        ChangedAutoDetect::Diff(value.to_string())
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

impl ChangedRootsResult {
    /// The roots the request asked for: the resolved ones, each query
    /// diagnostic's root, and each routine selector that resolved to none
    /// (`selector-unmatched`) or to several (`selector-ambiguous`). Round-2
    /// n08: a request naming one ambiguous routine printed `rootsRequested:
    /// 0` beside `changed.routines` holding it.
    #[must_use]
    pub fn roots_requested(&self, query_diagnostics: usize) -> usize {
        let unresolved_selectors = self
            .diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d,
                    ChangedRootsDiagnostic::SelectorUnmatched { .. }
                        | ChangedRootsDiagnostic::SelectorAmbiguous { .. }
                )
            })
            .count();
        self.roots.len() + query_diagnostics + unresolved_selectors
    }
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
    /// What changed paths are read against: the diff root and the workspace's
    /// anchors (#363).
    pub diff_root: PathRoot,
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

/// The root of an absolute path (#363): `/`, `//` (a `//server/share` path) or
/// a disk `X:/` (the letter as written).
#[derive(Debug, Clone, PartialEq, Eq)]
enum AbsRoot {
    Slash,
    Unc,
    Disk(char),
}

impl AbsRoot {
    /// One root, a disk letter in any case (a disk letter is not a folder name).
    fn same(&self, other: &AbsRoot) -> bool {
        match (self, other) {
            (AbsRoot::Disk(a), AbsRoot::Disk(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        }
    }

    fn spelling(&self) -> String {
        match self {
            AbsRoot::Slash => "/".into(),
            AbsRoot::Unc => "//".into(),
            AbsRoot::Disk(c) => format!("{c}:/"),
        }
    }
}

/// `path`'s root and its folders (`\` read as `/`, `.` and empty segments
/// dropped), or `None` for a relative path. A drive-relative `C:x.al` or a bare
/// `C:` has no root (#363 spec §2).
fn split_rooted(path: &str) -> Option<(AbsRoot, Vec<&str>)> {
    let (root, rest) = if let Some(rest) = path.strip_prefix("//") {
        (AbsRoot::Unc, rest)
    } else if let Some(rest) = path.strip_prefix('/') {
        (AbsRoot::Slash, rest)
    } else {
        let b = path.as_bytes();
        if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
            (AbsRoot::Disk(b[0] as char), &path[3..])
        } else {
            return None;
        }
    };
    let folders = rest
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    Some((root, folders))
}

/// The workspace's absolute location (#363): a root and its folders, case kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    root: AbsRoot,
    folders: Vec<String>,
}

impl Anchor {
    /// The anchor `spelling` names (`/home/x/ws`, `//srv/s/ws`, `C:/Git/ws`, a
    /// root-only `/`, `//` or `C:/`; `\` read as `/`). `None` for a relative
    /// spelling or one with a `..` folder: `..` is never cancelled (#76), so such
    /// an anchor would match nothing.
    pub fn parse(spelling: &str) -> Option<Anchor> {
        let s = spelling.replace('\\', "/");
        let (root, folders) = split_rooted(&s)?;
        if folders.contains(&"..") {
            return None;
        }
        Some(Anchor {
            root,
            folders: folders.into_iter().map(str::to_string).collect(),
        })
    }

    /// A workspace path's anchor: a Windows `\\?\` disk prefix is removed
    /// (`\\?\U:\Git\x` → `U:/Git/x`); a `\\?\UNC\` path gives none.
    pub fn of_path(path: &std::path::Path) -> Option<Anchor> {
        let s = path.to_string_lossy();
        if s.starts_with(r"\\?\UNC\") {
            return None;
        }
        Anchor::parse(s.strip_prefix(r"\\?\").unwrap_or(&s))
    }

    /// The anchor as [`Anchor::parse`] reads it back.
    pub fn spelling(&self) -> String {
        format!("{}{}", self.root.spelling(), self.folders.join("/"))
    }

    /// The workspace key `path` names below this anchor: the rest, lowercased.
    /// `None` when `path` is relative, has another root, does not start with the
    /// anchor's folders ([`same_folder`]), or IS the anchor (no rest).
    fn strip(&self, path: &str) -> Option<String> {
        let path = path.replace('\\', "/");
        let (root, parts) = split_rooted(&path)?;
        if !self.root.same(&root) || parts.len() <= self.folders.len() {
            return None;
        }
        // `..` is never cancelled (#76): such a path names no anchored file.
        let below = !parts.contains(&"..")
            && self
                .folders
                .iter()
                .zip(&parts)
                .all(|(want, got)| same_folder(got, want));
        below.then(|| parts[self.folders.len()..].join("/").to_lowercase())
    }

    /// Whether `path` is this anchor or a folder above it.
    fn path_at_or_above(&self, path: &str) -> bool {
        let path = path.replace('\\', "/");
        let Some((root, parts)) = split_rooted(&path) else {
            return false;
        };
        self.root.same(&root)
            && parts.len() <= self.folders.len()
            && parts
                .iter()
                .zip(&self.folders)
                .all(|(got, want)| same_folder(got, want))
    }
}

/// Whether `path` is absolute in the anchor grammar (`/`, `//`, `X:/`, `\` read
/// as `/`).
pub(crate) fn is_rooted_path(path: &str) -> bool {
    split_rooted(&path.replace('\\', "/")).is_some()
}

/// The workspace's anchors, in order (#363 spec §2): the lexical location, then
/// the canonical one when it differs. The ONE builder: `decide_diff_root` and
/// nothing else makes anchors from the file system.
#[must_use]
pub fn anchors_of(
    lexical: Option<&std::path::Path>,
    canonical: Option<&std::path::Path>,
) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::new();
    for a in [lexical, canonical]
        .into_iter()
        .flatten()
        .filter_map(Anchor::of_path)
    {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// What changed paths are read against (#363): the [`DiffRoot`] and the
/// workspace's absolute anchors. A path below an anchor has exactly one
/// reading, the rest; every other path reads under `root` as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathRoot {
    pub root: DiffRoot,
    pub anchors: Vec<Anchor>,
}

impl From<DiffRoot> for PathRoot {
    fn from(root: DiffRoot) -> Self {
        PathRoot {
            root,
            anchors: Vec::new(),
        }
    }
}

impl PathRoot {
    /// The anchors' spellings (the wire's `pathAnchors`).
    #[must_use]
    pub fn anchor_spellings(&self) -> Vec<String> {
        self.anchors.iter().map(Anchor::spelling).collect()
    }

    /// `root` with the anchors `spellings` name, in order; a spelling that names
    /// none is dropped.
    #[must_use]
    pub fn with_spellings(root: DiffRoot, spellings: &[String]) -> PathRoot {
        PathRoot {
            root,
            anchors: spellings.iter().filter_map(|s| Anchor::parse(s)).collect(),
        }
    }

    /// The one reading below an anchor (the first that matches), if any.
    fn anchored(&self, path: &str) -> Option<String> {
        self.anchors.iter().find_map(|a| a.strip(path))
    }
}

/// A diff-root decision as the wire carries it (#363 spec §4): the root in its
/// old shape, its anchors beside it (`pathAnchors`).
#[must_use]
pub fn decision_to_wire(
    decided: &Option<Result<PathRoot, String>>,
) -> (Option<Result<DiffRoot, String>>, Vec<String>) {
    match decided {
        None => (None, Vec::new()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
        Some(Ok(p)) => (Some(Ok(p.root.clone())), p.anchor_spellings()),
    }
}

/// [`decision_to_wire`]'s inverse; a message with no anchors (an older peer's)
/// gives none.
#[must_use]
pub fn decision_from_wire(
    root: Option<Result<DiffRoot, String>>,
    anchors: &[String],
) -> Option<Result<PathRoot, String>> {
    root.map(|r| r.map(|root| PathRoot::with_spellings(root, anchors)))
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
fn diff_readings(files: &FileKeys<'_>, root: &PathRoot, path: &str) -> Vec<String> {
    path_readings(root, path)
        .filter(|k| files.contains_key(k))
        .collect()
}

/// Every key `path` may name under `root`, file or not. Below an anchor: that
/// one reading, the rest (#363), so an absolute path is never ambiguous.
/// Otherwise as written, then below the git top level.
fn path_readings(root: &PathRoot, path: &str) -> impl Iterator<Item = String> {
    let as_written = || Some(normalize_input(path));
    let readings = match (root.anchored(path), &root.root) {
        (Some(rest), _) => [Some(rest), None],
        (None, DiffRoot::Workspace) => [as_written(), None],
        (None, DiffRoot::Explicit(p)) => [strip_diff_prefix(p, path), None],
        (None, DiffRoot::GitTop(p)) => [as_written(), strip_diff_prefix(p, path)],
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
/// workspace has a file. An absolute `path` that is the workspace or a folder
/// above it counts too (#363, the anchors).
fn holds_workspace_files(
    folders: &std::collections::HashSet<String>,
    any_file: bool,
    root: &PathRoot,
    path: &str,
) -> bool {
    path_readings(root, path).any(|r| folders.contains(&r))
        || (any_file
            && (at_or_above_prefix(&root.root, path)
                || root.anchors.iter().any(|a| a.path_at_or_above(path))))
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
    root: &PathRoot,
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
    root: &PathRoot,
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
    root: &PathRoot,
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
                owner_display: owners.object_key(&r.stable_routine_id),
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
        let mut matches = narrow_to_owner(base, matched_form, matches, |id| owners.object_key(id));
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
                    |id| owners.object_key(id),
                    selector,
                ),
            });
        } else if matches.len() >= 2
            && !crate::engine::l5::selector_index::declarations_of_one_routine(&matches)
        {
            // candidates = `<owner>::<display>` (display falls back to the id).
            // Round-2 G40: ordered by where each is declared (file, line), so
            // every rendering lists an ambiguity in source order; the sort is
            // stable, so ids with no row keep their identity-table order.
            let mut matches = matches;
            matches.sort_by_key(|id| {
                rows.get(id.as_str()).map(|r| {
                    (
                        r.source_anchor.source_unit_id.clone(),
                        r.source_anchor.start_line,
                    )
                })
            });
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
            // Every declaration of the id is touched (a selector names no
            // line), and so is every declaration of a split routine
            // (`split_decls`): a name names the routine, not one build of it.
            for id in matches {
                hits.extend_from_slice(spans.of_routine(&id));
                roots.insert(id);
            }
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
    /// Round-4 literal routes: the unresolved sites of the cone left out
    /// because no route from the root to them can run.
    pub unresolved_literal_dead: usize,
    /// r4 final (m02, c15): the unresolved interface calls of the cone left
    /// out because every route from the root fixes their implementation by a
    /// literal enum value (not open world).
    pub unresolved_literal_narrowed: usize,
    pub unresolved_traversal: UnresolvedTraversal,
    pub entry_diagnostics: Vec<EntryDiagnostic>,
    /// An Interface / ControlAddIn procedure declaration (#71): no body, no effects.
    pub declaration_only: bool,
    /// B4/L16: the commits no route runs as a commit, with why
    /// ([`crate::engine::l5::digest::DigestEntryResult::suppressed_commits`]).
    pub suppressed_commits: Vec<crate::engine::l5::digest::SuppressedCommit>,
    /// ENG-10 ([`crate::engine::l5::digest::DigestEntryResult::try_root_refusal`]).
    pub try_root_refusal: bool,
}

/// ENG-09: how a human output names a checked run's own commit, whose anchor
/// is the `Codeunit.Run` (where BC commits, at the run's end), never a
/// `Commit()`.
pub(crate) const RUN_COMMIT_LABEL: &str = "COMMIT (checked Codeunit.Run)";

/// ENG-09: whether a row is a checked run's own commit (its
/// `detail.resourceId` is `capability_cone::RUN_COMMIT`).
/// ENG-22: the error a checked run raises itself when a write is pending at
/// it (resource `checked-run` on an implicit error).
pub(crate) const RUN_ERROR_LABEL: &str = "ERROR (checked Codeunit.Run with a write pending)";

pub(crate) fn is_run_error_row(effect_type: &str, detail: &[(String, String)]) -> bool {
    effect_type == "ERROR_IMPLICIT"
        && detail
            .iter()
            .any(|(k, v)| k == "resourceId" && v == crate::engine::l4::capability_cone::RUN_COMMIT)
}

/// ENG-10: the error a try call raises when the server refuses a database
/// write inside it (resource `write-in-try` on an implicit error).
pub(crate) const TRY_REFUSAL_LABEL: &str = "ERROR (database write refused inside a try call)";

pub(crate) fn is_try_refusal_row(effect_type: &str, detail: &[(String, String)]) -> bool {
    effect_type == "ERROR_IMPLICIT"
        && detail.iter().any(|(k, v)| {
            k == "resourceId" && v == crate::engine::l4::capability_cone::WRITE_IN_TRY
        })
}

pub(crate) fn is_run_commit_row(e: &DigestEffectFull) -> bool {
    e.effect_type == "COMMIT"
        && e.detail
            .iter()
            .any(|(k, v)| k == "resourceId" && v == crate::engine::l4::capability_cone::RUN_COMMIT)
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
    /// r3 E2: `DigestEffectResult`'s field of the same name. Not serialized.
    pub(crate) flow_notes: Option<Box<crate::engine::l5::literal_reach::FlowNotes>>,
    /// W2-2: a boundary group's crossing (design §3.3); `None` off a group.
    /// The rollup groups rows by it; the 3.x digest serializes it once per
    /// entry (`entries[].crossings`).
    pub crossing: Option<Arc<crate::engine::l5::digest::Crossing>>,
}

impl DigestEffectFull {
    /// The row's site, `file:line` (1-based), or its file alone.
    pub(crate) fn site_text(&self) -> Option<String> {
        let f = self.evidence.file.as_ref()?;
        Some(match self.evidence.line {
            Some(l) => format!("{f}:{}", crate::engine::line_base::shown(l)),
            None => f.clone(),
        })
    }
}

/// r3 §7 R1: the loaded-subscriber assumptions of `rows` of `effects`, one
/// record per assumption with the rows' sites it rules out.
pub(crate) fn assumption_records(
    effects: &[DigestEffectFull],
    rows: impl IntoIterator<Item = usize>,
) -> Vec<crate::engine::l5::literal_reach::AssumptionRecord> {
    crate::engine::l5::literal_reach::AssumptionRecord::collect(rows.into_iter().flat_map(|i| {
        let e = &effects[i];
        let site = e.site_text();
        e.flow_notes
            .iter()
            .flat_map(|n| &n.assumed)
            .map(move |k| (k, site.clone()))
    }))
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
    /// Round-4 literal routes: how many of the cone's unresolved sites the
    /// literal arguments rule out (left out of `items`).
    pub literal_dead: usize,
    /// r4 final (m02, c15): how many of the cone's unresolved interface
    /// calls run only with an implementation the literal arguments fix (left
    /// out of `items`: not open world).
    pub literal_narrowed: usize,
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
    /// ENG-06: the cone visits a routine whose record operation may hold any
    /// table and was kept closed because no loaded table could run anything
    /// there (`ResolvedCalls::loaded_only_tables`): a `no` holds only within
    /// the loaded apps. `None` off prove's plan, as `gaps_in_cone`.
    pub any_table_loaded_only: Option<bool>,
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
        ReachWorkers::SERIAL,
    );
    out.pop().expect("one reader, one result")
}

/// TLS-22: how many roots the literal pass runs at once (1: serial on the
/// calling thread, no pool). A parameter on every route into the pass, never
/// read from the environment inside the library. In tests it also carries a
/// [`PassProbe`] down the route, so a test observes the pass it set up.
#[derive(Clone, Debug)]
pub struct ReachWorkers {
    n: usize,
    #[cfg(test)]
    probe: Option<std::sync::Arc<PassProbe>>,
}

impl ReachWorkers {
    /// One root at a time, on the calling thread.
    pub const SERIAL: ReachWorkers = ReachWorkers {
        n: 1,
        #[cfg(test)]
        probe: None,
    };

    /// `n` roots at once (at least 1).
    #[must_use]
    pub fn new(n: usize) -> ReachWorkers {
        ReachWorkers {
            n: n.max(1),
            #[cfg(test)]
            probe: None,
        }
    }

    /// How many roots run at once.
    #[must_use]
    pub fn count(&self) -> usize {
        self.n
    }

    /// Tests: the same setting, observed by `probe`.
    #[cfg(test)]
    pub(crate) fn with_probe(&self, probe: std::sync::Arc<PassProbe>) -> ReachWorkers {
        ReachWorkers {
            n: self.n,
            probe: Some(probe),
        }
    }

    /// Tests: the probe this setting carries.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> Option<&std::sync::Arc<PassProbe>> {
        self.probe.as_ref()
    }
}

/// The literal pass's worker count (spec 2026-11-11 section 8.5): fixed, not
/// the machine's core count, so peak memory is the same on any machine with
/// at least this many cores. Measured on CDO (2026-11-12, release-fast, 22
/// cores): `prove.reach_effects` of the 699-root `impact --diff` 87.1 s at
/// 1, 51.3 s at 2, 35.5 s at 4, 29.4 s at 8, 35.6 s at 16 (peak RSS +5% at
/// 8, +15% at 16); `docs/superpowers/notes/2026-11-12-parallel-literal-pass.md`.
pub(crate) const REACH_WORKERS: usize = 8;

static DEFAULT_REACH_WORKERS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Set once by the binary at start (`AL_SEM_REACH_WORKERS`, for
/// measurement); tests never set it.
pub fn set_default_reach_workers(n: usize) {
    let _ = DEFAULT_REACH_WORKERS.set(n.max(1));
}

/// [`REACH_WORKERS`] on a machine of `cores`: never more workers than cores,
/// so a one-core host runs every pass serially (no pool).
fn capped_by_cores(cores: usize) -> usize {
    REACH_WORKERS.min(cores).max(1)
}

/// The setting of a one-shot CLI read (`alsem impact`, `digest`, `prove`):
/// the binary's count, or [`REACH_WORKERS`] capped by the cores. The
/// resident engine runs serially instead ([`crate::engine::resident`]'s
/// revisions; owner decision 2026-11-12, q-20261112T032637-1269d8fc).
pub fn default_reach_workers() -> ReachWorkers {
    ReachWorkers::new(*DEFAULT_REACH_WORKERS.get_or_init(|| {
        capped_by_cores(std::thread::available_parallelism().map_or(1, usize::from))
    }))
}

/// TLS-22: the pool of exactly `n` threads the literal pass runs `n` roots
/// in, one per distinct `n` for the process (production uses one count),
/// with the big stacks the searches' recursion needs.
pub(crate) fn reach_pool(n: usize) -> &'static rayon::ThreadPool {
    static POOLS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<usize, &'static rayon::ThreadPool>>,
    > = std::sync::OnceLock::new();
    let mut pools = POOLS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    pools.entry(n).or_insert_with(|| {
        Box::leak(Box::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .stack_size(crate::big_stack::BIG_STACK_SIZE)
                .thread_name(|i| format!("al-sem-reach-{i}"))
                .build()
                .expect("the literal pass's thread pool"),
        ))
    })
}

/// Tests (TLS-22): what one route's literal passes did: the most roots in
/// flight at once, and the passes run. `first_wave`: hold each of the first
/// `n` roots to start until `n` are in flight or the timeout passes (later
/// roots never wait), so a pass with `n` workers shows `n` at once, and a
/// smaller pool shows fewer instead of hanging.
/// `suffix_hits`: the passes' suffix-cache hits (a tree served again).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct PassProbe {
    in_flight: std::sync::atomic::AtomicUsize,
    started: std::sync::atomic::AtomicUsize,
    pub(crate) max_in_flight: std::sync::atomic::AtomicUsize,
    pub(crate) passes: std::sync::atomic::AtomicUsize,
    pub(crate) suffix_hits: std::sync::atomic::AtomicUsize,
    pub(crate) first_wave: std::sync::Mutex<Option<(usize, std::time::Duration)>>,
    /// TLS-21, the up walk: tasks run, scratches made, the pool size each
    /// task saw (`rayon::current_num_threads`).
    pub(crate) up_tasks: std::sync::atomic::AtomicUsize,
    pub(crate) up_done: std::sync::atomic::AtomicUsize,
    pub(crate) scratches_made: std::sync::atomic::AtomicUsize,
    pub(crate) pool_threads: std::sync::Mutex<Vec<usize>>,
    /// Hold the first root's walk until every other root's walk is done (or
    /// the timeout passes), so it finishes last.
    pub(crate) hold_first_root: std::sync::Mutex<Option<std::time::Duration>>,
}

/// A root in flight ([`PassProbe::enter`]); leaves on drop.
#[cfg(test)]
pub(crate) struct InFlight<'p>(&'p PassProbe);

#[cfg(test)]
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
impl PassProbe {
    pub(crate) fn enter(&self) -> InFlight<'_> {
        use std::sync::atomic::Ordering::SeqCst;
        let k = self.started.fetch_add(1, SeqCst);
        let now = self.in_flight.fetch_add(1, SeqCst) + 1;
        self.max_in_flight.fetch_max(now, SeqCst);
        let wave = *self.first_wave.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((n, timeout)) = wave
            && k < n
        {
            let until = std::time::Instant::now() + timeout;
            while self.in_flight.load(SeqCst) < n && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        InFlight(self)
    }

    /// The most roots in flight at once.
    pub(crate) fn max(&self) -> usize {
        self.max_in_flight.load(std::sync::atomic::Ordering::SeqCst)
    }
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
/// `workers`: the literal pass's (TLS-22).
#[allow(clippy::too_many_arguments)]
pub(crate) fn digest_effects_each(
    tree: &CapabilitySnapshot,
    model: &Model,
    readers: &[&[String]],
    tests: bool,
    gaps_in_cone: bool,
    include_reads: bool,
    boundary: BoundaryMode,
    workers: ReachWorkers,
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
    let entries = passed_root_entries(tree, model, &all, include_reads, boundary, workers)
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
    workers: ReachWorkers,
) -> Vec<Option<DigestEntryResult>> {
    let mut entries = root_entries(tree, model, roots, include_reads, boundary);
    let present: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].is_some())
        .collect();
    let mut passed: Vec<DigestEntryResult> = entries.iter_mut().filter_map(Option::take).collect();
    literal_reach_pass(tree, model, &mut passed, workers);
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
    /// The model the tree was built from: the call sites of the cone items
    /// the tree holds no evidence for ([`FinishCx::site_of`]).
    model: &'t Model,
    /// The model's routine rows by stable id, built on the first item that
    /// needs one.
    rows_by_stable: std::sync::OnceLock<HashMap<&'t str, Vec<&'t ModelRoutine>>>,
    /// TLS-17: the unresolved cones' snapshot indexes, built on the first
    /// root that needs a cone and read for every root of the read (a
    /// `FinishCx` lives for one read, every batch of a streamed one).
    cone_indexes: std::sync::OnceLock<ConeIndexes<'t>>,
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
            model,
            rows_by_stable: std::sync::OnceLock::new(),
            cone_indexes: std::sync::OnceLock::new(),
        }
    }

    /// The anchor (`file`, line, column) of call site `callsite_id` in the
    /// model rows of the stable routine `owning` (engine limits B8,
    /// [`crate::engine::l5::unresolved_cone::place_items`]).
    /// `root`'s raw unresolved cone, off the read's indexes (built once,
    /// TLS-17). A test can switch it to the verbatim pre-TLS-17 search
    /// ([`LEGACY_CONES`]), so the finished output of both can be compared.
    fn raw_cone(&self, root: &str) -> crate::engine::l5::unresolved_cone::UnresolvedConeResult {
        #[cfg(test)]
        if LEGACY_CONES.with(std::cell::Cell::get) {
            return crate::engine::l5::unresolved_cone::unresolved_cone_legacy(
                self.tree,
                root,
                DEFAULT_MAX_DEPTH,
                DEFAULT_MAX_VISITED,
            );
        }
        let idx = self
            .cone_indexes
            .get_or_init(|| ConeIndexes::new(self.tree));
        unresolved_cone_in(self.tree, idx, root, DEFAULT_MAX_DEPTH, DEFAULT_MAX_VISITED)
    }

    fn site_of(&self, owning: &str, callsite_id: &str) -> Option<(String, u32, u32)> {
        let rows = self.rows_by_stable.get_or_init(|| {
            let mut m: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
            for r in &self.model.workspace.routines {
                m.entry(r.stable_routine_id.as_str()).or_default().push(r);
            }
            m
        });
        rows.get(owning)?
            .iter()
            .flat_map(|r| &r.call_sites)
            .find(|cs| cs.id == callsite_id)
            .map(|cs| {
                let a = &cs.source_anchor;
                (a.source_unit_id.clone(), a.start_line, a.start_column)
            })
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
                        let mut c = self.raw_cone(r);
                        // Engine limits B8: an item the tree holds no
                        // evidence for takes its model call site's anchor.
                        crate::engine::l5::unresolved_cone::place_items(&mut c.items, |o, cs| {
                            self.site_of(o, cs)
                        });
                        // Round-4 literal routes: the sites the literal
                        // pass ruled out from this root leave the cone (as
                        // a gap and as a blocker); their count stays.
                        let own = entries.iter().find(|e| e.routine_id == *r);
                        let dead = own.map_or(&[][..], |e| e.literal_dead_sites.as_slice());
                        let before = c.items.len();
                        c.items
                            .retain(|i| dead.binary_search(&i.callsite_id).is_err());
                        let literal_dead = before - c.items.len();
                        // r4 final: and the interface calls whose
                        // implementation the literal arguments fix on
                        // every route: not open world.
                        let narrowed = own.map_or(&[][..], |e| e.literal_narrowed_sites.as_slice());
                        let before = c.items.len();
                        c.items
                            .retain(|i| narrowed.binary_search(&i.callsite_id).is_err());
                        let literal_narrowed = before - c.items.len();
                        (
                            r.clone(),
                            UnresolvedCone {
                                literal_dead,
                                literal_narrowed,
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
                                any_table_loaded_only: gaps_in_cone.then(|| {
                                    let only = &self.model.calls.loaded_only_tables;
                                    !only.is_empty()
                                        && c.visited_ids.iter().any(|v| only.contains(v))
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
    /// Tests: [`FinishCx`]'s raw cones from the verbatim pre-TLS-17 search
    /// (`unresolved_cone_legacy`) instead of the read's shared indexes.
    pub(crate) static LEGACY_CONES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
    workers: ReachWorkers,
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
    let mut pass = LiteralPass::new(tree, model, workers);
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
    workers: ReachWorkers,
) {
    LiteralPass::new(tree, model, workers).pass(entries);
}

/// [`literal_reach_pass`]'s per-tree half: the literal reach, the group
/// paths and the suffix trees, built once and shared by every batch of
/// entries [`LiteralPass::pass`] is given (a streamed read,
/// [`digest_effects_chunks`]).
pub(crate) struct LiteralPass<'t> {
    reach: crate::engine::l5::literal_reach::LiteralReach<'t>,
    group_paths: crate::engine::l5::group_path::GroupPaths<'t>,
    suffixes: crate::engine::l5::literal_reach::SuffixCache<'t>,
    /// Round-4 literal routes: the tree's unresolved call sites (the ledger
    /// rows an unresolved cone lists), sorted and distinct.
    unresolved_sites: Vec<&'t str>,
    /// TLS-22: how many roots run at once.
    workers: ReachWorkers,
}

impl<'t> LiteralPass<'t> {
    pub(crate) fn new(
        tree: &'t CapabilitySnapshot,
        model: &'t Model,
        workers: ReachWorkers,
    ) -> LiteralPass<'t> {
        let reach = {
            let _s = crate::engine::perf_trace::span("digest", "prove.literal_reach");
            crate::engine::l5::literal_reach::LiteralReach::new(tree, model)
        };
        // G35: how each group resource is reached inside the dependency.
        let group_paths = {
            let _s = crate::engine::perf_trace::span("digest", "prove.group_paths");
            crate::engine::l5::group_path::GroupPaths::new(&tree.typed_edges, model)
        };
        let mut unresolved_sites: Vec<&str> = tree
            .callsite_resolutions
            .iter()
            .filter(|r| crate::engine::l5::unresolved_cone::is_listed(r))
            .map(|r| r.callsite_id.as_str())
            .collect();
        unresolved_sites.sort_unstable();
        unresolved_sites.dedup();
        LiteralPass {
            reach,
            group_paths,
            // W2-0b (design §3.4.3): the suffix trees, shared by every root,
            // crossing and resource of this read.
            suffixes: crate::engine::l5::literal_reach::SuffixCache::default(),
            unresolved_sites,
            workers,
        }
    }

    /// The pass over `entries` (see [`literal_reach_pass`]).
    pub(crate) fn pass(&mut self, entries: &mut [DigestEntryResult]) {
        let LiteralPass {
            reach,
            group_paths,
            suffixes,
            unresolved_sites,
            workers,
        } = self;
        literal_reach_entries(
            reach,
            group_paths,
            suffixes,
            unresolved_sites,
            entries,
            workers,
        );
    }
}

/// TLS-22: the read's suffix-cache work, for `ALSEM_TRACE` (a duplicate is a
/// tree two workers built at once).
impl Drop for LiteralPass<'_> {
    fn drop(&mut self) {
        let s = self.suffixes.stats();
        crate::engine::perf_trace::counter_delta("digest.suffix_built", s.built as u64);
        crate::engine::perf_trace::counter_delta("digest.suffix_duplicate", s.duplicate as u64);
        crate::engine::perf_trace::counter_delta("digest.suffix_evicted", s.evicted as u64);
        #[cfg(test)]
        if let Some(p) = &self.workers.probe {
            p.suffix_hits
                .fetch_add(s.hits, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// B7/L23: the effect types a trap acts on: an error (`Error`, an implicit
/// error) and the error dialog it shows (`UI_ERROR`, the same site): a
/// caught error shows no dialog.
pub(crate) fn is_error_row_type(t: &str) -> bool {
    matches!(t, "ERROR_THROW" | "ERROR_IMPLICIT" | "UI_ERROR")
}

/// B7/L23 + L30 (engine limits wave 2): rule each error row of `entry` (an
/// `Error`, an implicit error, its dialog) against the traps on its routes
/// ([`crate::engine::l5::traps`]), into the row's
/// [`FlowNotes::trap`](crate::engine::l5::literal_reach::FlowNotes): caught
/// on every route, escaping only with a write pending at an isolated event,
/// or escaping while its first route is trapped. And name the routines on
/// its route that collect errors (L30, `FlowNotes::collected_by`).
///
/// Only a row that is ONE site is ruled (its type's site search completed,
/// not a boundary group): a row that may stand for unseen sites keeps
/// escaping. Only completed searches rule: a capped one leaves every row as
/// it was. A root with no trap and no collector in its reach pays one scan
/// of the trap table and nothing more.
fn judge_traps<'t>(
    reach: &crate::engine::l5::literal_reach::LiteralReach<'t>,
    root_tree: &mut Option<Option<crate::engine::l5::literal_reach::LiveTree<'t>>>,
    entry: &mut DigestEntryResult,
) {
    let partial = |t: &str| entry.partial_site_types.iter().any(|p| p == t);
    // A boundary group (errors beyond a call into dependency code) is ruled
    // by its crossing's site: every error beyond it is caught when every
    // route to the crossing crosses a trap. (A crossing edge that is itself
    // a trap is not ruled here: such a group stays escaping.)
    let rows: Vec<usize> = (0..entry.effects.len())
        .filter(|&i| {
            let e = &entry.effects[i];
            !e.literal_unreachable
                && is_error_row_type(&e.effect_type)
                && (e.boundary_group || !partial(&e.effect_type))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let Some(full) = root_tree
        .get_or_insert_with(|| reach.live_tree_of(&entry.routine_id))
        .as_ref()
    else {
        return;
    };
    // ENG-22: a checked run's own error is ruled by whether a write may be
    // pending at the run, so its rows need the root's run points.
    let has_run_error = rows.iter().any(|&i| {
        !entry.effects[i].boundary_group
            && is_run_error_row(&entry.effects[i].effect_type, &entry.effects[i].detail)
    });
    let runs = if has_run_error {
        reach.run_points(&entry.routine_id)
    } else {
        None
    };
    let scope = reach.trap_scope(&entry.routine_id, full, runs.as_ref());
    let collectors = reach.collectors_reached(full);
    // ENG-10: a try call's refusal error is ruled by its own reach (no try
    // call catches it), and a group caught only by a try call may hold one.
    let refusal_open = if reach.has_try_calls() {
        reach.refusal_reach(&entry.routine_id)
    } else {
        None
    };
    let has_refusal = reach.has_try_calls()
        && rows.iter().any(|&i| {
            let e = &entry.effects[i];
            e.boundary_group || is_try_refusal_row(&e.effect_type, &e.detail)
        });
    if scope.is_none() && !collectors && !has_run_error && !has_refusal {
        return;
    }
    let in_try = if has_refusal {
        reach.in_try_reach(&entry.routine_id)
    } else {
        None
    };
    for i in rows {
        let e = &mut entry.effects[i];
        let ops: Vec<String> = if e.boundary_group {
            // The crossing's own site (an event crossing has none: not ruled).
            let site = match &e.crossing {
                Some(c) => c.site.guarded_id(),
                None => e
                    .evidence_callsite_id
                    .as_deref()
                    .or(e.evidence_operation_id.as_deref()),
            };
            match site {
                Some(s) => vec![s.to_string()],
                None => continue,
            }
        } else {
            let mut ops: Vec<String> = e
                .evidence_operation_id
                .iter()
                .chain(&e.evidence_callsite_id)
                .cloned()
                .collect();
            for op in &e.origin_operation_ids {
                if !ops.contains(op) {
                    ops.push(op.clone());
                }
            }
            ops
        };
        let mut trap = scope
            .as_ref()
            .and_then(|s| reach.trap_verdict(full, s, &ops)); // B7 review: a caught ruling that may rest on a checked run assumes
        // the root's callers have no write pending.
        let caught = matches!(
            trap,
            Some(crate::engine::l5::literal_reach::TrapVerdict::Caught(_))
        );
        let mut assumes_clean_entry = caught && scope.as_ref().is_some_and(|s| s.clean_checked);
        // Round 2: ... and, when a `Commit()` before the run ends the write,
        // that the callers set no `[CommitBehavior]` that stops it.
        let mut assumes_normal_commit = caught && scope.as_ref().is_some_and(|s| s.clean_by_commit);
        // ENG-22: the run's own error, unless a trap outside the run catches
        // it on every route: not raised where no write may be pending at the
        // run (resting on the callers' entry), otherwise raised if one is.
        // A trap on some routes only keeps its own words (the row stays
        // escaping, never unconditional).
        if !caught
            && trap.is_none()
            && !e.boundary_group
            && is_run_error_row(&e.effect_type, &e.detail)
            && let Some(site) = e.evidence_callsite_id.as_deref()
            && let Some((v, clean)) = reach.run_error_verdict(runs.as_ref(), site)
        {
            trap = Some(v);
            if let Some(by_commit) = clean {
                assumes_clean_entry = true;
                assumes_normal_commit = by_commit;
            }
        }
        // ENG-10: the try call's own refusal error: whatever the ordinary
        // traps said (an outer try call does not catch it).
        if !e.boundary_group
            && is_try_refusal_row(&e.effect_type, &e.detail)
            && let Some(site) = e.evidence_callsite_id.as_deref()
            && let Some(v) =
                reach.try_refusal_verdict(&entry.routine_id, refusal_open.as_ref(), site)
        {
            trap = Some(v);
            assumes_clean_entry = false;
            assumes_normal_commit = false;
        }
        // ENG-10: a group a try call catches may hold the refusal error of a
        // try call inside the dependency code (T14: no try call around it
        // catches it): when a crossing edge runs inside a try call on a route
        // with no checked run or `asserterror`, and its target may make such
        // a try call (`crossing_in_try`).
        // Whatever trap the witness route named (another route may enter a
        // try call).
        // Only an implicit-error group can hold a refusal (`write-in-try`).
        if e.boundary_group
            && e.effect_type == "ERROR_IMPLICIT"
            && matches!(
                &trap,
                Some(crate::engine::l5::literal_reach::TrapVerdict::Caught(_))
            )
            && let (Some(c), Some(it)) = (e.crossing.as_ref(), in_try.as_ref())
            && let Some((o, s)) = c
                .edges
                .iter()
                .find_map(|x| reach.crossing_in_try(it, &x.from, x.site.guarded_id(), &x.to))
        {
            trap = Some(
                crate::engine::l5::literal_reach::TrapVerdict::UnlessWritePending(
                    reach.try_call_at(o, s),
                ),
            );
            assumes_clean_entry = false;
            assumes_normal_commit = false;
        }
        let collected_by = if collectors && e.effect_type == "ERROR_THROW" && !e.boundary_group {
            reach
                .tree_witness(full, &ops)
                .map(|(steps, _)| reach.collectors_on(&entry.routine_id, &steps))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if trap.is_some() || !collected_by.is_empty() {
            let notes = e.flow_notes.get_or_insert_with(Default::default);
            notes.trap = trap;
            notes.trap_assumes_clean_entry = assumes_clean_entry;
            notes.trap_assumes_normal_commit = assumes_normal_commit;
            notes.collected_by = collected_by;
        }
    }
}

fn literal_reach_entries<'t>(
    reach: &crate::engine::l5::literal_reach::LiteralReach<'t>,
    group_paths: &crate::engine::l5::group_path::GroupPaths<'t>,
    suffixes: &crate::engine::l5::literal_reach::SuffixCache<'t>,
    unresolved_sites: &[&str],
    entries: &mut [DigestEntryResult],
    workers: &ReachWorkers,
) {
    // On the calling thread around the whole pass, whatever runs the roots.
    let _s = crate::engine::perf_trace::span("digest", "prove.reach_effects");
    // A shared fill is never cancelled by a request (context-server design
    // §4.6), so nothing here asks to stop; the check is wired for a reader
    // that owns its search.
    let never = || false;
    // TLS-22: one root's pass. It writes only its own entry; what it shares
    // with other roots (`reach`, `group_paths`, `suffixes`) is read-only or a
    // pure memo (spec 2026-11-11 section 8.3).
    let one = |entry: &mut DigestEntryResult| {
        #[cfg(test)]
        let _in_flight = workers.probe.as_deref().map(PassProbe::enter);
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
                    // B4/L16: a group of commits that raise an error ends
                    // on a commit.
                    let ty = if e.detail.iter().any(|(k, _)| k == "commitBehavior") {
                        "COMMIT"
                    } else {
                        e.effect_type.as_str()
                    };
                    // ENG-09: a group of checked runs' commits ends on one;
                    // ENG-22 review: so does a group of their own errors.
                    let run = crate::engine::l4::capability_cone::RUN_COMMIT;
                    let marker = (matches!(ty, "COMMIT" | "ERROR_IMPLICIT")
                        && e.detail.iter().any(|(k, v)| k == "resourceId" && v == run))
                    .then_some(run);
                    let ops = vec![(String::new(), reach.terminal_ops((ty, marker)))];
                    e.group_terminal_path = group_paths
                        .paths(reach, tree, entries, e, &ops, suffixes, &never)
                        .pop()
                        .map(|(_, o)| o);
                }
                // r3 §7 R5: the guards of the row's own terminal witness,
                // through the dependency entry to the operation. A resource's
                // witness keeps its own (`GroupPath::guards`): prove prints
                // them with the resource's path.
                let witness = e
                    .group_terminal_path
                    .as_ref()
                    .and_then(|w| w.as_ref().ok()?.guards.clone());
                if let Some(set) = witness {
                    let notes = e.flow_notes.get_or_insert_with(Default::default);
                    notes.beyond_unanalyzed = set.unanalyzed > 0;
                    notes.guards = Some(set.guards);
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
        let (dead, dead_resources, assumed, dead_sites, narrowed) = reach
            .unreachable_effects_noted(
                &entry.routine_id,
                &entry.effects,
                &per_site,
                unresolved_sites,
            );
        // Round-4 literal routes: an unresolved site no route from the root
        // can reach is no gap and no blocker of this root.
        entry.literal_dead_sites = dead_sites;
        // r4 final: nor is an interface call whose implementation every
        // route's literal arguments fix (the search follows only that one).
        entry.literal_narrowed_sites = narrowed;
        for i in dead {
            entry.effects[i].literal_unreachable = true;
        }
        // r3 E2: the rows dropped only under the loaded-subscribers
        // assumption name it.
        for (i, keeps) in assumed {
            entry.effects[i]
                .flow_notes
                .get_or_insert_with(Default::default)
                .assumed = keeps;
        }
        // B7/L23 + L30: error rows behind traps, and under error collection.
        judge_traps(reach, &mut root_tree, entry);
        // ENG-10: a root that is itself a try function may be refused.
        entry.try_root_refusal = reach.try_root_may_refuse(&entry.routine_id);
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
            // r3 E2: the guards on the route a reader prints (the shortest
            // listed one), each evaluated or not under the route's values.
            // r3 §7 R5: a group with no witness through its entry (neither
            // its own nor a resource's): the guards up to the entry, the
            // rest not analyzed.
            let witnessed = e
                .group_terminal_path
                .iter()
                .chain(e.group_resource_paths.iter().map(|(_, w)| w))
                .any(|w| w.as_ref().is_ok_and(|w| w.guards.is_some()));
            if e.boundary_group
                && !witnessed
                && e.flow_notes.as_ref().is_none_or(|n| n.guards.is_none())
                && let Some(p) = e
                    .via_paths
                    .iter()
                    .filter(|p| !p.is_empty())
                    .min_by_key(|p| p.len())
            {
                let guards = reach.route_guards(&entry.routine_id, p, &[]);
                let notes = e.flow_notes.get_or_insert_with(Default::default);
                notes.guards = Some(guards);
                notes.beyond_unanalyzed = true;
            }
            if !e.boundary_group
                && let Some(p) = e
                    .via_paths
                    .iter()
                    .filter(|p| !p.is_empty())
                    .min_by_key(|p| p.len())
                    .or_else(|| e.via_paths.first())
            {
                // The evidence site, by operation or (an implicit error of a
                // call such as `Evaluate`, a COMMIT) by call site: without
                // it the terminal site's own guards were lost (r4 final c05:
                // `if EqualPos > 0` around the `Evaluate` at :278).
                let mut ops: Vec<String> = e
                    .evidence_operation_id
                    .iter()
                    .chain(&e.evidence_callsite_id)
                    .cloned()
                    .collect();
                for op in &e.origin_operation_ids {
                    if !ops.contains(op) {
                        ops.push(op.clone());
                    }
                }
                let guards = reach.route_guards(&entry.routine_id, p, &ops);
                e.flow_notes.get_or_insert_with(Default::default).guards = Some(guards);
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
        // r3 E2: a COMMIT row too (n01: the first-ranked commit's listed
        // paths could not run, so its witness and its guards came from none).
        for e in entry.effects.iter_mut() {
            if e.boundary_group
                || e.literal_unreachable
                || e.via_paths.iter().any(|p| !p.is_empty())
                || !matches!(
                    e.effect_type.as_str(),
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" | "COMMIT"
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
            // r3 E2: the searched witness's guards, as a listed route's.
            let guards = match tree {
                None => ops
                    .iter()
                    .find(|op| reach.live_in_root(&entry.routine_id, op))
                    .map(|op| reach.route_guards_steps(&entry.routine_id, &[], op)),
                Some(Some(t)) => reach
                    .tree_witness(t, &ops)
                    .map(|(steps, op)| reach.route_guards_steps(&entry.routine_id, &steps, op)),
                Some(None) => None,
            };
            if let Some(g) = guards {
                e.flow_notes.get_or_insert_with(Default::default).guards = Some(g);
            }
            e.own_path = Some(match tree {
                None => group_paths.own_path_in_root(reach, &entry.routine_id, &ops),
                Some(t) => group_paths.own_path(reach, t, &ops),
            });
        }
    };
    #[cfg(test)]
    if let Some(p) = &workers.probe {
        p.passes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    // Serial when asked, and from inside a rayon worker: there `install`
    // would let the blocked worker run other jobs while it waits, which
    // deadlocks when one takes a lock this thread holds (`fill_witness` holds
    // `scratch` across the pass); such a caller is parallel already (a test
    // reading many fixtures on rayon).
    if workers.n <= 1 {
        entries.iter_mut().for_each(one);
        return;
    }
    if rayon::current_thread_index().is_some() {
        crate::engine::perf_trace::counter_delta("digest.reach_serial_in_rayon", 1);
        entries.iter_mut().for_each(one);
        return;
    }
    use rayon::prelude::*;
    // One root per task: per-root cost is uneven, work stealing balances it.
    reach_pool(workers.n).install(|| entries.par_iter_mut().with_max_len(1).for_each(one));
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
                unresolved_literal_dead: 0,
                unresolved_literal_narrowed: 0,
                unresolved_traversal: UnresolvedTraversal {
                    truncated: false,
                    max_depth: DEFAULT_MAX_DEPTH,
                    visited_routines: 0,
                    dependency_routines: 0,
                },
                entry_diagnostics: Vec::new(),
                declaration_only: true,
                suppressed_commits: Vec::new(),
                try_root_refusal: false,
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
        let mut suppressed_commits = Vec::new();
        let mut try_root_refusal = false;

        if let Some(entry) = base_entry {
            suppressed_commits = std::mem::take(&mut entry.suppressed_commits);
            try_root_refusal = entry.try_root_refusal;
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
                    flow_notes: eff.flow_notes.take(),
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
            unresolved_literal_dead: cone.literal_dead,
            unresolved_literal_narrowed: cone.literal_narrowed,
            unresolved_traversal: cone.traversal,
            entry_diagnostics,
            declaration_only: false,
            suppressed_commits,
            try_root_refusal,
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

/// B4/L16: one suppressed commit as a sentence, for prove, digest and
/// explain: `COMMIT at <file:line> is ignored (CommitBehavior Ignore on …)`,
/// or `raises an error instead (…)`; a commit beyond a crossing is named by
/// the crossing.
#[must_use]
pub fn suppressed_commit_words(s: &crate::engine::l5::digest::SuppressedCommit) -> String {
    let at = match (&s.file, s.line) {
        (Some(f), Some(l)) => format!(" at {f}:{}", crate::engine::line_base::shown(l)),
        (Some(f), None) => format!(" at {f}"),
        _ => String::new(),
    };
    let what = match &s.beyond {
        Some(b) => format!("the COMMITs beyond {b}, entered{at},"),
        None => format!("COMMIT{at}"),
    };
    let (verb_one, verb_many) = if s.behavior == "Error" {
        ("raises an error instead", "raise an error instead")
    } else {
        ("is ignored", "are ignored")
    };
    let verb = if s.beyond.is_some() {
        verb_many
    } else {
        verb_one
    };
    format!("{what} {verb} ({})", s.reason)
}

/// B4/L16: one suppressed commit's JSON (`behavior`, `file`, `line`
/// 1-based, `beyond`, `reason`).
#[must_use]
pub fn suppressed_commit_value(
    s: &crate::engine::l5::digest::SuppressedCommit,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("behavior".into(), s.behavior.into());
    if let Some(f) = &s.file {
        m.insert("file".into(), f.clone().into());
    }
    if let Some(l) = s.line {
        m.insert("line".into(), crate::engine::line_base::shown(l).into());
    }
    if let Some(b) = &s.beyond {
        m.insert("beyond".into(), b.clone().into());
    }
    m.insert("reason".into(), s.reason.clone().into());
    serde_json::Value::Object(m)
}

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
    // Round-4 literal routes: the unresolved sites left out because no route
    // from the root to them can run under its literal arguments. Present
    // only then.
    if entry.unresolved_literal_dead > 0 {
        m.insert(
            "unresolvedLiteralDead".into(),
            (entry.unresolved_literal_dead as u64).into(),
        );
    }
    // r4 final: the interface calls left out because the literal arguments
    // fix their implementation on every route. Present only then.
    if entry.unresolved_literal_narrowed > 0 {
        m.insert(
            "unresolvedLiteralNarrowed".into(),
            (entry.unresolved_literal_narrowed as u64).into(),
        );
    }

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
    // B4/L16 (additive): the commits a `[CommitBehavior]` ignores or turns
    // into an error.
    if !entry.suppressed_commits.is_empty() {
        m.insert(
            "suppressedCommits".into(),
            entry
                .suppressed_commits
                .iter()
                .map(suppressed_commit_value)
                .collect(),
        );
    }

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
            // Round-2 G40 (additive here; digest 5.0.0's one candidate
            // array): each candidate's selector, display, signature, stable
            // id and declaration, in source order — every 4.x string list is
            // rebuilt from it (`digest_v5::to_v4`).
            if !listed.is_empty()
                && routines.len() == listed.len()
                && listed.iter().all(|c| !c.at.is_empty())
            {
                m.insert(
                    "candidateRows".into(),
                    listed
                        .iter()
                        .zip(candidates)
                        .map(|(c, display)| {
                            serde_json::json!({
                                "selector": c.selector,
                                "display": display,
                                "signature": c.signature(),
                                "stableId": c.stable_id,
                                "definedAt": c.at,
                            })
                        })
                        .collect(),
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
    lookup_next: Option<serde_json::Value>,
    v3: Option<V3Inputs<'_>>,
) -> String {
    // payload.summary
    // rootsRequested = roots + query diagnostics + unresolved routine selectors
    // (`ChangedRootsResult::roots_requested`).
    let roots_requested = changed_roots_result.roots_requested(query_diagnostics.len());
    let roots_resolved = entries.len();
    let total_effects: usize = entries.iter().map(|e| e.effects.len()).sum();
    let total_unresolved: usize = entries.iter().map(|e| e.unresolved.len()).sum();

    let mut summary = serde_json::Map::new();
    summary.insert("rootsRequested".into(), (roots_requested as u64).into());
    summary.insert("rootsResolved".into(), (roots_resolved as u64).into());
    summary.insert("totalEffects".into(), (total_effects as u64).into());
    summary.insert("totalUnresolved".into(), (total_unresolved as u64).into());
    // r3 §7 R2 (additive, 5.0.0 only; the 4.0.0 adapter drops it): a lookup
    // failure's ready call.
    if let Some(next) = lookup_next
        && v3.as_ref().is_some_and(|v| v.v5)
    {
        summary.insert("next".into(), next);
    }

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
    // Round-3 n08/m01: with no root resolved there is no routine table or
    // path cap to describe.
    if let (Some(table), Some(v), false) = (routines_table, &v3, entries.is_empty()) {
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
    // Round-2 G44 (additive): the fields that change run to run.
    env.insert(
        "volatile".into(),
        crate::engine::result_header::volatile(deterministic),
    );
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
                    // Round-2 G42 (additive): the resolved table in prove's
                    // own shape beside the asked `table`.
                    let values: Vec<serde_json::Value> = values
                        .iter()
                        .map(|a| {
                            let mut a = a.clone();
                            if let (Some(v), Some(t)) = (v3.as_ref(), a["table"].as_str()) {
                                a["tableRef"] =
                                    crate::engine::l5::prove::table_ref(v.cx.model, v.cx, t);
                            }
                            a
                        })
                        .collect();
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
    // 4.0.0 (output validation V5, §7.3): what several roots share, once
    // (`digest_v4`); `--schema 3` keeps the 3.x document for one release.
    if v3.as_ref().is_some_and(|v| v.v4) {
        doc = sort_keys_keep_nulls(crate::engine::l5::digest_v4::document_from_v3(doc));
    }
    // 5.0.0 (round-2 output validation §4.6): one shape per field and the
    // §8.1 boundary words (`digest_v5`); `--schema 4` keeps 4.0.0 for one
    // release.
    if v3.as_ref().is_some_and(|v| v.v4 && v.v5) {
        doc = sort_keys_keep_nulls(crate::engine::l5::digest_v5::document_from_v4(doc));
    } else if let Some(p) = doc.get_mut("payload") {
        // r3 §7 R1: `assumptionRecords` is 5.x's; `--schema 4` and 3 keep
        // their documents as they were.
        crate::engine::l5::digest_v5::strip_assumption_records(p);
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
    /// Output validation V5 (§7.3): the 4.0.0 document (`digest_v4`);
    /// `false` is `--schema 3`.
    pub v4: bool,
    /// Round-2 output validation §4.6: the 5.0.0 document (`digest_v5`)
    /// over the 4.0.0 one; `false` is `--schema 4` (or 3).
    pub v5: bool,
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
            // The effect sections; a run of hops several of their paths
            // share prints once, under a segment id (output validation §7.5).
            let mut effects = rr::commit_lines(r, entry, cx, opts);
            effects.extend(rr::workspace_write_lines(r, entry, cx, opts, &|_| None));
            effects.extend(rr::caller_dependent_lines(r, entry, cx));
            effects.extend(rr::beyond_lines(r, entry, cx, opts));
            effects.extend(rr::no_path_lines(r, entry, cx, opts));
            effects.extend(rr::literal_dead_lines(r, opts));
            effects.extend(rr::temporary_lines(r, opts));
            effects.extend(rr::event_lines(r, cx, opts));
            body.extend(crate::engine::path_segments::segment(effects));
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
    changed: &[ChangedRootsDiagnostic],
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
    // r3 H5: a lookup failure says what it is: an ambiguous selector names
    // how many routines it names, an unmatched one is `not found`.
    let ambiguous = changed.iter().find_map(|d| match d {
        ChangedRootsDiagnostic::SelectorAmbiguous { candidates, .. } => Some(candidates.len()),
        _ => None,
    });
    let unmatched = changed
        .iter()
        .any(|d| matches!(d, ChangedRootsDiagnostic::SelectorUnmatched { .. }));
    h.answer = match &facts[..] {
        [] if let Some(n) = ambiguous => format!(
            "ambiguous — names {}",
            crate::engine::l5::routine_rollup::plural(n, "routine", "routines")
        ),
        [] if unmatched => "not found".to_string(),
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
        // B7 review round 2: what a root's caught errors rest on.
        for a in &f.assumptions {
            if !h.assumptions.contains(a) {
                h.assumptions.push(a.clone());
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
            .push(format!("{} lists every list in full", Input::Detail.on(s)));
    }
    // r3 H5: with no root resolved there is no effect row to ask for.
    if !facts.is_empty() {
        h.more
            .push(format!("{} for every effect row", Input::FormatJson.on(s)));
    }
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
    // W2-2 (T-W2-13): the one RollupTotals serialization; r3 §5.2: in the
    // envelope summary and the index, its dependency boundary in 5.0.0's
    // words (`dependencyCalls`, never `entryPoints`).
    let mut totals = r.totals.to_json();
    crate::engine::l4::query_v5::rename_keys(&mut totals, crate::engine::l4::query_v5::RENAMES);
    let mut v = json!({
        "routine": entry_display(cx, e),
        "stableId": e.routine_id,
        "analysis": facts.analysis.key(),
        "answer": facts.answer(),
        // Round 2 G10: the summary row's short answer (the full one stays
        // in the result index).
        "shortAnswer": short_answer(&r.totals, &facts.analysis),
        "effects": e.effects.len(),
        "effectsByType": by_type,
        // Round-2 G42: the effect counts by type under the name that is
        // always a map (`effectsByType` leaves in the next digest major).
        "effectCounts": by_type,
        "summary": totals,
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
    // Round 2 G13: when 3 tables or fewer are observed written, the row
    // names them.
    if (1..=3).contains(&r.may_write.len()) {
        v["observedWrites"] = json!(r.may_write.values().collect::<Vec<_>>());
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
                    // Round-2 G42: one table shape, prove's (`tableRef`:
                    // `{id, name, number, app, appId}`, `null` for a name
                    // that resolves to no table) beside what was asked.
                    let asked = a["table"].as_str().unwrap_or("");
                    let mut t = json!({
                        "asked": asked,
                        "tableRef": crate::engine::l5::prove::table_ref(cx.model, cx, asked),
                        "answer": a.get("answer").cloned().unwrap_or(serde_json::Value::Null),
                        "blockers": blockers,
                    });
                    let blockers = a["blockedBy"].as_array().map_or(&[][..], Vec::as_slice);
                    if let Some(b) = first_blocker(blockers) {
                        t["firstBlocker"] = b;
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

/// A table answer's `firstBlocker` (round 2 G13): the first blocker with a
/// source site, `{kind, detail, site: "file:line"}`; when no blocker has a
/// site, the first blocker as it is, with `site: null`.
fn first_blocker(blockers: &[serde_json::Value]) -> Option<serde_json::Value> {
    let site_of = |b: &serde_json::Value| -> Option<String> {
        let a = b.get("anchor")?;
        let file = a["file"].as_str()?;
        Some(match a["line"].as_u64() {
            Some(l) => format!("{file}:{l}"),
            None => file.to_string(),
        })
    };
    let b = blockers
        .iter()
        .find(|b| site_of(b).is_some())
        .or_else(|| blockers.first())?;
    Some(serde_json::json!({
        "kind": b["kind"],
        "detail": b["detail"],
        "site": site_of(b),
    }))
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
    let head = json!({
        "kind": "digest",
        "rootsRequested": roots_requested,
        "rootsResolved": entries.len(),
        "totalEffects": entries.iter().map(|e| e.effects.len()).sum::<usize>(),
        "totalUnresolved": total_unresolved,
        "changedRootsDiagnostics": changed_roots_diagnostics,
    });
    let test_scope = test_coverage.map(|tc| {
        let mut s = json!({
            "status": tc.coverage.status,
            "reasons": tc.coverage.reasons,
            "testProceduresConsidered": tc.test_procedures_considered,
            "testAppsNotLoaded": tc.test_apps_not_loaded.iter().map(|a| a.name.clone())
                .collect::<Vec<_>>(),
        });
        // Round 2 G9: the hand-over to the tests tool.
        if let Some(next) = &tc.next {
            s["next"] = next.json();
        }
        s
    });
    compact_digest_summary(head, rows, test_scope)
}

/// The keys of a full summary row the result index keeps (round 2 G10):
/// the identity, the full answer, the verdicts, the test status, and the
/// totals that carry conditional persistence and uncertainty.
const INDEX_ROW_KEYS: &[&str] = &[
    "routine",
    "stableId",
    "selector",
    "signature",
    "member",
    "analysis",
    "answer",
    "tableAnswers",
    "testStatus",
    "effects",
    // r3 §5.1 (A P10): `effectsByType` is `effectCounts` by construction
    // (one map under two names); the index keeps the one that stays.
    "effectCounts",
    "summary",
    "guaranteeLabels",
    "observedWrites",
];

/// A short summary answer longer than this is factored into `shared` when
/// two rows or more give it (overloads mostly do).
const SHARED_ANSWER_MIN: usize = 24;

/// The digest summary from its `head` (the totals), the full `rows` (one per
/// root, [`digest_entry_summary`]) and the test scope (round 2 G10). Each
/// listed row is compact: `index` (its 1-based row in the result index,
/// which holds its signature, stable id, full answer and totals), `selector`,
/// `analysis`, `testStatus`, `tableAnswers` and the short `answer`, one clause
/// per effect family with its qualifiers; an answer two rows share is in
/// `shared`. When the rows still do not fit 8 KiB, the overflow contract
/// holds: `summaryTruncated` and the complete index
/// ([`crate::engine::result_index::with_index`]).
fn compact_digest_summary(
    mut head: serde_json::Value,
    rows: Vec<serde_json::Value>,
    test_scope: Option<serde_json::Value>,
) -> serde_json::Value {
    use serde_json::json;
    let next = test_scope.as_ref().and_then(|s| s.get("next")).cloned();
    let index_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut x = json!({});
            for k in INDEX_ROW_KEYS {
                if let Some(v) = r.get(*k) {
                    x[*k] = v.clone();
                }
            }
            // r3 §5.2: a row's sentence is its overall effect (every
            // family, as the envelope's row says it); the writes-only
            // summary line stays beside it.
            if let Some(a) = r.get("shortAnswer") {
                x["writesAnswer"] = r["answer"].clone();
                x["answer"] = a.clone();
            }
            x
        })
        .collect();
    // r3 §5.2: the tests hand-off once per index (every row whose test
    // status is `not loaded` would repeat it).
    let not_loaded = rows
        .iter()
        .filter(|r| r["testStatus"].as_str() == Some("not loaded"))
        .count();
    let index_shared = next.filter(|_| not_loaded > 0).map(
        |n| json!({"testsNext": n, "testsNextFor": "every row whose testStatus is not loaded"}),
    );
    // F top-9 (c25): the test status most rows share, once at the summary's
    // top level; a listed row names its own only where it differs (the
    // index keeps every row's).
    let mut status_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        if let Some(s) = r["testStatus"].as_str() {
            *status_counts.entry(s).or_default() += 1;
        }
    }
    let default_status: Option<String> = status_counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(s, _)| (*s).to_string());
    // The short answers two rows or more share, once.
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        if let Some(a) = r["shortAnswer"].as_str() {
            *counts.entry(a).or_default() += 1;
        }
    }
    let mut shared = serde_json::Map::new();
    let mut key_of: BTreeMap<&str, String> = BTreeMap::new();
    for r in &rows {
        let Some(a) = r["shortAnswer"].as_str() else {
            continue;
        };
        if counts[a] > 1 && a.len() > SHARED_ANSWER_MIN && !key_of.contains_key(a) {
            let key = format!("s{}", shared.len() + 1);
            shared.insert(key.clone(), json!({ "answer": a }));
            key_of.insert(a, key);
        }
    }
    let listed: Vec<serde_json::Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut x = json!({ "index": i + 1 });
            for k in [
                "selector",
                "analysis",
                "testStatus",
                "tableAnswers",
                "observedWrites",
            ] {
                if let Some(v) = r.get(k) {
                    if k == "testStatus" && v.as_str() == default_status.as_deref() {
                        continue;
                    }
                    x[k] = v.clone();
                }
            }
            // Round-2 G42: a shared answer is named by `sharedWith` (one
            // key, one meaning), never by a polymorphic field.
            match r["shortAnswer"].as_str().and_then(|a| key_of.get(a)) {
                Some(key) => x["sharedWith"] = json!(key),
                None => x["answer"] = r["shortAnswer"].clone(),
            }
            x
        })
        .collect();
    head["routinesListed"] = json!(listed.len());
    head["rowDetail"] = json!(
        "compact: each row names its result-index row (`index`), which holds its signature, \
         stable id, full answer and totals"
    );
    head["shared"] = serde_json::Value::Object(shared);
    if let Some(s) = &default_status {
        head["testStatus"] = json!(s);
    }
    head["routines"] = serde_json::Value::Array(listed);
    if let Some(s) = test_scope {
        head["testScope"] = s;
    }
    let mut v = crate::engine::result_index::with_index(head, "routines", "index", index_rows);
    v["routinesListed"] = json!(v["routines"].as_array().map_or(0, Vec::len));
    if let Some(s) = index_shared {
        v[crate::engine::result_index::SHARED_KEY] = s;
    }
    v
}

/// The summary row's short answer (round 2 G10): one clause per effect
/// family, each with its qualifiers — the writes with who owns the tables
/// and the caller-dependent ones' condition, commits, the effect kinds with
/// their counts, and the analysis with its reasons when partial.
fn short_answer(
    t: &crate::engine::l5::routine_rollup::RollupTotals,
    analysis: &crate::engine::result_header::Analysis,
) -> String {
    use crate::engine::l5::routine_rollup::{FAMILIES, plural, thousands};
    let mut parts: Vec<String> = Vec::new();
    let mut writes: Vec<String> = Vec::new();
    if t.may_write_tables > 0 {
        let mut q: Vec<String> = Vec::new();
        if t.owned_by_app > 0 {
            q.push(format!("{} owned by this app", thousands(t.owned_by_app)));
        }
        if t.by_dependencies > 0 {
            q.push(format!("{} by dependencies", thousands(t.by_dependencies)));
        }
        writes.push(format!(
            "may write {} ({})",
            plural(t.may_write_tables, "table", "tables"),
            q.join(", ")
        ));
    }
    if t.unresolved_table_writes > 0 {
        writes.push(format!(
            "{} on a table not resolved",
            plural(t.unresolved_table_writes, "write", "writes")
        ));
    }
    if t.caller_dependent_tables > 0 {
        writes.push(format!(
            "caller-dependent writes on {} (only when the caller's record is not temporary)",
            plural(t.caller_dependent_tables, "table", "tables")
        ));
    }
    let complete = matches!(analysis, crate::engine::result_header::Analysis::Complete);
    parts.push(if writes.is_empty() && complete {
        "writes: no".to_string()
    } else if writes.is_empty() {
        "writes: none observed".to_string()
    } else {
        writes.join(", ")
    });
    // r3 §5.2: the word table's absence words, never the legacy key
    // (`no observed`).
    parts.push(format!(
        "commits: {}",
        match t.commits {
            "no observed" => "none observed",
            "none" => "no (every candidate decided)",
            c => c,
        }
    ));
    for (key, label) in FAMILIES {
        if let Some(n) = t.kinds.get(*key).filter(|n| **n > 0) {
            let least = if t.kinds_partial.contains(*key) {
                "at least "
            } else {
                ""
            };
            parts.push(format!("{label} {least}{}", thousands(*n)));
        }
    }
    if !complete {
        parts.push(analysis.human());
    }
    parts.join("; ")
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
            // shortcut: in-process callers (tests only) get no anchors (#363 spec §9).
            diff_root: PathRoot::default(),
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
            diff_root: PathRoot::default(),
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
    /// What `diff_text`'s and `changed_files`' paths are relative to (#77, #339),
    /// with the workspace's anchors (#363).
    pub diff_root: PathRoot,
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
    // rootsRequested as `payload.summary` counts it (roots, query diagnostics
    // and unresolved routine selectors).
    let roots_requested = changed_roots.roots_requested(query_diagnostics.len());
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
        // Round 2 G9: no test code here and a sibling test app found —
        // the complete tests call that selects from it, on the same input
        // (the routines asked, or the files: given, or the roots' own).
        if tc.test_procedures_considered == 0 && !tc.test_apps_not_loaded.is_empty() {
            let fcx = crate::engine::follow_up::Cx::new(render.surface, req.single_app);
            let next = crate::engine::follow_up::FollowUp::new(
                crate::engine::follow_up::Tool::Tests,
                fcx,
                "selects the tests of the sibling test apps this engine does not load",
            );
            tc.next = Some(match (&changed_routines, &changed_files) {
                (Some(rs), _) => next.arg("routines", rs.clone()),
                (None, Some(fs)) => next.arg("files", fs.clone()),
                (None, None) => {
                    let files: BTreeSet<String> = entries
                        .iter()
                        .filter_map(|e| e.routine_anchor.as_ref()?.file.as_deref())
                        .filter_map(|f| f.strip_prefix("ws:"))
                        .map(str::to_string)
                        .collect();
                    next.arg("files", files.into_iter().collect::<Vec<_>>())
                }
            });
        }
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
        &changed_roots.diagnostics,
    );

    // r3 §7 R2: a lookup failure's ready call, in the JSON document's
    // `payload.summary.next` (5.0.0), the envelope's summary and the human
    // text alike.
    let lookup = if exit_code == 2 {
        lookup_next(
            &changed_roots.diagnostics,
            crate::engine::follow_up::Cx::new(render.surface, req.single_app),
        )
    } else {
        LookupNext::default()
    };

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
        lookup.call.as_ref().map(|c| c.json()),
        (!render.schema2).then_some(V3Inputs {
            rollups: &rollups,
            cx: &rollup_cx,
            opts: render.opts(),
            compact: render.compact,
            table_answers: &table_answers,
            scope: Some(header.scope_json(serde_json::Map::new())),
            v4: !render.schema3,
            v5: !render.schema3 && !render.schema4,
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
        human_text.push_str(&t.human_line(render.surface));
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
    let mut human_text = dependencies.readable_locations(human_text);
    drop(rollup_cx);
    cx.release(Cell::L4);
    cx.release(Cell::Effects);

    // §6.3 (round-2 n08): a no-answer's outcome from its changed-roots
    // diagnostics' kinds (`selector-ambiguous` → `ambiguous`).
    let mut summary = summary;
    // Round-3 n08/m01: a lookup failure carries no effect-analysis totals,
    // and its ready call (r3 §7 R2: an ambiguity's complete call, a typo's
    // corrected one) in the human text and as `summary.next`.
    if exit_code == 2 {
        let mut slim = serde_json::Map::new();
        for k in [
            "kind",
            "rootsRequested",
            "rootsResolved",
            "changedRootsDiagnostics",
        ] {
            if let Some(v) = summary.get(k) {
                slim.insert(k.into(), v.clone());
            }
        }
        if let Some(n) = &lookup.call {
            slim.insert("next".into(), n.json());
        }
        for l in &lookup.human {
            human_text.push_str(l);
            human_text.push('\n');
        }
        summary = serde_json::Value::Object(slim);
    }
    if exit_code == 2
        && let Some(o) = crate::engine::result_header::outcome_of_kinds(
            changed_roots
                .diagnostics
                .iter()
                .map(ChangedRootsDiagnostic::kind),
        )
        && let serde_json::Value::Object(m) = &mut summary
    {
        m.insert("outcome".into(), o.into());
    }

    DigestRunResult {
        json_text,
        human_text,
        exit_code,
        summary,
    }
}

/// What a digest lookup failure (no root resolved) offers (round-3 n08/m01;
/// output validation r3 §7 R2): the ready call (`summary.next`, in the JSON
/// document's `payload.summary` and the envelope's summary) and the human
/// lines that end the answer.
#[derive(Debug, Default)]
pub(crate) struct LookupNext {
    pub(crate) call: Option<crate::engine::follow_up::FollowUp>,
    pub(crate) human: Vec<String>,
}

/// r3 §7 R2: a lookup failure's ready call, one rule for the two failures.
///
/// **Ambiguous** (any selector names several routines): the complete digest
/// call over every resolved candidate of the ambiguous selectors (each with
/// a non-empty selector), and the human lines `complete call: …` and
/// `differs in: …` (explain's c26 form; one `differs in` per ambiguous
/// selector, named when there are several).
///
/// **Not found** (no selector is ambiguous): per unmatched selector its
/// nearest name, labelled `did you mean: …`; the corrected call names those
/// nearest names, and only when every unmatched selector has one (a call
/// that drops a selector is not a correction). A selector with no near name
/// gets no correction. Never a call with an empty target, and the nearest
/// names are never put in one call as if they were overloads of one name.
pub(crate) fn lookup_next(
    changed: &[ChangedRootsDiagnostic],
    fcx: crate::engine::follow_up::Cx,
) -> LookupNext {
    use crate::engine::follow_up::{FollowUp, Tool};
    let ambiguous: Vec<(
        &str,
        &[crate::engine::l5::selector_index::AmbiguousCandidate],
    )> = changed
        .iter()
        .filter_map(|d| match d {
            ChangedRootsDiagnostic::SelectorAmbiguous {
                selector, listed, ..
            } => Some((selector.as_str(), listed.as_slice())),
            _ => None,
        })
        .collect();
    let mut out = LookupNext::default();
    if !ambiguous.is_empty() {
        let selectors: Vec<String> = ambiguous
            .iter()
            .flat_map(|(_, l)| l.iter())
            .map(|c| c.selector.clone())
            .filter(|s| !s.is_empty())
            .collect();
        if selectors.is_empty() {
            return out;
        }
        let call = FollowUp::new(Tool::Digest, fcx, "every candidate").arg("routines", selectors);
        out.human
            .push(format!("  complete call: {}", call.text(fcx.surface)));
        for (sel, listed) in &ambiguous {
            if let Some(d) = crate::engine::l5::selector_index::differs_in(listed) {
                out.human.push(if ambiguous.len() > 1 {
                    format!("  differs in ('{}'): {d}", escape_controls(sel))
                } else {
                    format!("  differs in: {d}")
                });
            }
        }
        out.call = Some(call);
        return out;
    }
    let unmatched: Vec<Option<&crate::engine::l5::selector_index::NearName>> = changed
        .iter()
        .filter_map(|d| match d {
            ChangedRootsDiagnostic::SelectorUnmatched { hints, .. } => Some(
                hints
                    .nearest
                    .first()
                    .filter(|n| !n.selector.trim().is_empty()),
            ),
            _ => None,
        })
        .collect();
    // One selector: explain's and prove's one line (`did you mean: … —
    // corrected call: …`).
    if let [Some(n)] = unmatched[..] {
        out.human
            .push(crate::engine::l5::selector_index::corrected_call_line(
                Tool::Digest,
                fcx,
                &n.selector,
                n.overloads,
                &[],
            ));
        out.call = Some(
            FollowUp::new(Tool::Digest, fcx, "the nearest name")
                .arg("routines", vec![n.selector.clone()]),
        );
        return out;
    }
    for n in unmatched.iter().flatten() {
        out.human.push(format!(
            "  did you mean: {}{}",
            n.selector,
            if n.overloads > 1 {
                format!(
                    " (names {} overloads; the corrected call's answer lists each one's call)",
                    n.overloads
                )
            } else {
                String::new()
            }
        ));
    }
    if !unmatched.is_empty() && unmatched.iter().all(Option::is_some) {
        let call = FollowUp::new(Tool::Digest, fcx, "the nearest name").arg(
            "routines",
            unmatched
                .iter()
                .flatten()
                .map(|n| n.selector.clone())
                .collect::<Vec<_>>(),
        );
        out.human
            .push(format!("  corrected call: {}", call.text(fcx.surface)));
        out.call = Some(call);
    }
    out
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

    /// c25's 20 roots (round 2): the selectors after the owner, as the
    /// result index gave them.
    const C25_SELECTORS: [&str; 20] = [
        "BCSendMail",
        "SendMail(var Record \"CDO Document Template Line\", var Record \"CDO File\", var Record \"CDO Template Line Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean, Boolean, Boolean)",
        "SendMail(var Record \"CDO E-Mail Template Line\", var Record \"CDO File\", var Record \"CDO E-Mail Template Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean)",
        "SendMail(var Record \"CDO E-Mail Template Line\", var Record \"CDO File\", var Record \"CDO E-Mail Template Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean, Boolean, Boolean, Dictionary of [Code[10], Text])",
        "SetSendUsingBCEmailAccount",
        "SendMail(var Record \"CDO Document Template Line\", var Record \"CDO File\", var Record \"CDO Template Line Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean)",
        "SealDocumentsBeforeSending",
        "SendMSGraphMail",
        "SendMail(var Record \"CDO E-Mail Template Line\", var Record \"CDO File\", var Record \"CDO E-Mail Template Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean, Boolean, Boolean)",
        "CreateLog(var Record \"CDO Document Template Line\", var Record \"CDO File\", var Record \"CDO Template Line Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Boolean, Boolean)",
        "WaitForMailLimit",
        "StartEvents",
        "InsertImageAttachmentsAsBase64",
        "GetMailBodyAndCIDAttachemntsAsXmlDocument(var Record \"CDO Template Line Attachment\", Text)",
        "EndEvents",
        "SendMail(var Record \"CDO Document Template Line\", var Record \"CDO File\", var Record \"CDO Template Line Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Text, Boolean, Integer, Boolean, Boolean, Boolean, Boolean, Boolean, Boolean, Dictionary of [Code[10], Text])",
        "CreateLog(var Record \"CDO Document Template Line\", var Record \"CDO File\", var Record \"CDO Template Line Attachment\", var RecordRef, Text, Text, Text, Text, Text, Text, Boolean, Boolean, Dictionary of [Code[10], Text])",
        "SetSendUsingAzureApp",
        "GetMailBodyAndCIDAttachemntsAsXmlDocument(var Record \"CDO E-Mail Template Attachment\", Text)",
        "SetSendUsingDOSMTP",
    ];

    /// One full summary row of c25's shape: identity, a long full answer,
    /// the totals, and the short answer `short`.
    fn c25_row(i: usize, selector: &str, short: &str) -> serde_json::Value {
        let selector = format!("Codeunit \"CDO Send Mail Management\"::{selector}");
        serde_json::json!({
            "routine": selector,
            "selector": selector,
            "stableId": format!("f4b69b55-c90d-4937-8f53-2742898fa948:Codeunit:6175335#{i:064x}"),
            "signature": "(var Record \"CDO Document Template Line\", var Record \"CDO File\")",
            "analysis": "partial",
            "answer": "writes: may write 15 tables (4 owned by this app, 11 by dependencies) and 1 \
                       write on a table not resolved; 8 of them beyond Codeunit Email (8901)::Send \
                       (entered at ws:src/Features/Email/Sending/CDOSendMailManagement.Codeunit.al:260)",
            "shortAnswer": short,
            "effects": 221,
            "effectsByType": {"DB_INSERT": 8, "DB_MODIFY": 3, "ERROR_IMPLICIT": 27, "HTTP": 7},
            "summary": {"callerDependentTables": 1, "gaps": {"workspace": 3, "dependency": 14}},
            "testStatus": "not loaded",
        })
    }

    /// Round 2 G10: c25's 20 rows fit the 8 KiB summary in their compact
    /// form (every row listed, no `summaryTruncated`), each row naming its
    /// index row; the index keeps every row whole with its conditional
    /// persistence, uncertainty and test status. A wider selection still
    /// truncates under the overflow contract: `summaryTruncated`, and the
    /// complete index. Hand-stated rows (c25's selectors; short answers of
    /// the longest shapes the render gives, the six SendMail overloads
    /// sharing one).
    #[test]
    fn digest_summary_rows_fit_and_overflow_keeps_the_index() {
        use crate::engine::result_index::{INDEX_RESERVE_BYTES, ROWS_KEY};
        let long = "may write 15 tables (4 owned by this app, 11 by dependencies), 1 write on a \
                    table not resolved, caller-dependent writes on 1 table (only when the \
                    caller's record is not temporary); commits: may; UI 3; HTTP 2; isolated \
                    storage 4; errors 40; analysis partial (21 calls not resolved (6 in this \
                    workspace))";
        let short = |i: usize| -> String {
            match i % 4 {
                0 => "writes: none observed; commits: none".to_string(),
                1 => "may write 1 table (1 owned by this app); commits: no observed; errors 2; \
                      analysis partial (1 call not resolved)"
                    .to_string(),
                2 => format!(
                    "may write {} tables (2 owned by this app, 3 by dependencies); commits: may; errors 9; analysis partial (3 calls not resolved)",
                    5 + i
                ),
                _ => "writes: none observed; commits: none; UI 1".to_string(),
            }
        };
        let rows: Vec<serde_json::Value> = C25_SELECTORS
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let a = if s.starts_with("SendMail(") {
                    long.to_string()
                } else {
                    short(i)
                };
                c25_row(i, s, &a)
            })
            .collect();
        let head =
            || serde_json::json!({"kind": "digest", "rootsRequested": 20, "rootsResolved": 20});
        let scope = serde_json::json!({
            "status": "partial", "reasons": ["no-test-code", "test-apps-not-loaded"],
            "testProceduresConsidered": 0,
            "testAppsNotLoaded": ["Continia Document Output 365 - Test"],
            "next": {"tool": "alsem_tests", "arguments": {"files": ["src/Features/Email/Sending/CDOSendMailManagement.Codeunit.al"]},
                     "why": "selects the tests of the sibling test apps this engine does not load", "kind": "ordinary"},
        });
        let s = compact_digest_summary(head(), rows.clone(), Some(scope.clone()));
        let mut sent = s.clone();
        sent.as_object_mut().unwrap().remove(ROWS_KEY);
        let bytes = sent.to_string().len();
        assert!(
            bytes + INDEX_RESERVE_BYTES <= crate::engine::batch::output::SUMMARY_CAP_BYTES,
            "c25's 20 rows: {bytes} bytes"
        );
        assert!(s.get("summaryTruncated").is_none(), "{s:#}");
        assert_eq!(s["routines"].as_array().unwrap().len(), 20);
        assert_eq!(s["routines"][1]["index"], 2);
        assert!(s["routines"][1].get("stableId").is_none());
        let index = s[ROWS_KEY].as_array().unwrap();
        assert_eq!(index.len(), 20);
        assert_eq!(index[1]["summary"]["callerDependentTables"], 1);
        assert_eq!(index[1]["testStatus"], "not loaded");
        // r3 §5.2: the tests hand-off once per index, not on each row; a
        // row's sentence is its overall effect; one map per row.
        assert!(index[1].get("testsNext").is_none(), "{:#}", index[1]);
        let shared = &s[crate::engine::result_index::SHARED_KEY];
        assert_eq!(shared["testsNext"]["tool"], "alsem_tests", "{s:#}");
        assert_eq!(index[1]["answer"], rows[1]["shortAnswer"]);
        assert_eq!(index[1]["writesAnswer"], rows[1]["answer"]);
        assert!(index[1].get("effectsByType").is_none());
        // F top-9 (c25): the shared test status once; no listed row repeats it.
        assert_eq!(s["testStatus"], "not loaded", "{s:#}");
        assert!(
            s["routines"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r.get("testStatus").is_none()),
            "{s:#}"
        );
        // A wider selection: the summary truncates, the index holds all.
        let wide: Vec<serde_json::Value> = (0..80)
            .map(|i| c25_row(i, C25_SELECTORS[1], &format!("{long} #{i}")))
            .collect();
        let s = compact_digest_summary(head(), wide, Some(scope));
        let t = &s["summaryTruncated"];
        assert_eq!(t["rowsTotal"], 80, "{s:#}");
        assert!(t["rowsListed"].as_u64().unwrap() < 80);
        assert_eq!(s[ROWS_KEY].as_array().unwrap().len(), 80);
    }

    /// F top-9 (c25): the test status most rows share is the summary's
    /// `testStatus`; a listed row whose status differs names its own, and
    /// the index keeps every row's. Hand-stated: three rows `not loaded`,
    /// one `tests with a modeled path`.
    ///
    /// DISCRIMINATION (2026-11-08): leaving out every row's `testStatus`
    /// (`continue` for `testStatus` whatever its value) fails at "the row
    /// that differs"; restored, passes.
    #[test]
    fn a_shared_test_status_is_said_once_and_a_differing_row_keeps_its_own() {
        let mut rows: Vec<serde_json::Value> = (0..4)
            .map(|i| c25_row(i, C25_SELECTORS[i], "writes: none observed"))
            .collect();
        rows[2]["testStatus"] = "tests with a modeled path".into();
        let s = compact_digest_summary(serde_json::json!({"kind": "digest"}), rows, None);
        assert_eq!(s["testStatus"], "not loaded", "{s:#}");
        let listed = s["routines"].as_array().unwrap();
        assert_eq!(
            listed[2]["testStatus"], "tests with a modeled path",
            "the row that differs: {s:#}"
        );
        for i in [0, 1, 3] {
            assert!(listed[i].get("testStatus").is_none(), "{s:#}");
        }
        let index = s[crate::engine::result_index::ROWS_KEY].as_array().unwrap();
        assert_eq!(index[0]["testStatus"], "not loaded");
        assert_eq!(index[2]["testStatus"], "tests with a modeled path");
        assert!(
            s.get(crate::engine::result_index::SHARED_KEY).is_none(),
            "no hand-off without a test scope"
        );
    }

    /// Round 2 G13: a table answer's `firstBlocker` is the first blocker
    /// with a site, not the meta-entry listed before it; with no site at
    /// all, the first blocker with `site: null`.
    #[test]
    fn first_blocker_has_a_site() {
        let meta =
            serde_json::json!({"kind": "analysis-gap", "detail": "1 analysis gap(s) in cone"});
        let site = serde_json::json!({
            "kind": "open-world-dispatch",
            "detail": "Provider.GetCache().GetSubscription in GetLicenseState",
            "anchor": {"file": "dep:Continia Core:src/LicenseManagement.Codeunit.al", "line": 456},
        });
        let b = first_blocker(&[meta.clone(), site]).unwrap();
        assert_eq!(b["kind"], "open-world-dispatch");
        assert_eq!(
            b["site"],
            "dep:Continia Core:src/LicenseManagement.Codeunit.al:456"
        );
        let b = first_blocker(&[meta]).unwrap();
        assert_eq!(b["kind"], "analysis-gap");
        assert!(b["site"].is_null());
    }

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
                        diff_root: PathRoot::default(),
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
        // #363 S4: the relative compare never folds a disk letter.
        assert_eq!(strip_diff_prefix("C:", "c:/x.al"), None);
    }

    // --- #363 anchors ------------------------------------------------------

    fn anchor(s: &str) -> Anchor {
        Anchor::parse(s).unwrap_or_else(|| panic!("{s} is an anchor"))
    }

    /// #363 spec §3: the one reading below an anchor, and every non-match.
    #[test]
    fn anchor_strip_cases() {
        for (a, path, expected) in [
            ("/t/repo/Cloud", "/t/repo/Cloud/src/X.al", Some("src/x.al")),
            (
                "/t/repo/Cloud",
                "/t/repo/Cloud/./src//x.al",
                Some("src/x.al"),
            ),
            ("/t/repo/Cloud", "/t/repo/Cloud", None), // empty rest
            ("/t/repo/Cloud", "/t/repo/cloud/src/x.al", None), // folder case
            ("/t/repo/Cloud", "/t/repo/Cloudy/x.al", None),
            ("/t/repo/Cloud", "t/repo/Cloud/src/x.al", None), // relative
            ("/t/repo/Cloud", "/t/repo/Cloud/../Cloud/x.al", None), // `..` kept
            ("/", "/src/x.al", Some("src/x.al")),             // root-only
            ("//srv/share/ws", "//srv/share/ws/x.al", Some("x.al")),
            ("//srv/share/ws", "/srv/share/ws/x.al", None), // another root
            ("C:/Git/ws", "c:/Git/ws/x.al", Some("x.al")),  // only the disk folds
            ("C:/Git/ws", r"C:\Git\ws\src\x.al", Some("src/x.al")),
            ("C:/Git/ws", "C:/GIT/ws/x.al", None),
            ("C:/Git/ws", "C:x.al", None), // drive-relative: no root
            ("C:/", "C:/x.al", Some("x.al")),
            ("C:/", "C:", None),
        ] {
            assert_eq!(anchor(a).strip(path).as_deref(), expected, "{a} {path}");
        }
    }

    /// #363: what is no anchor, and the spellings that round-trip.
    #[test]
    fn anchor_parse_and_spelling() {
        for s in ["/", "//", "C:/", "/home/x/ws", "//srv/s/ws", "C:/Git/ws"] {
            assert_eq!(anchor(s).spelling(), s, "{s}");
        }
        assert_eq!(anchor(r"U:\Git\x").spelling(), "U:/Git/x");
        for s in ["ws", "C:", "C:x", "/t/../ws", ""] {
            assert_eq!(Anchor::parse(s), None, "{s}");
        }
        let p = |s: &str| Anchor::of_path(std::path::Path::new(s));
        assert_eq!(
            p(r"\\?\U:\Git\x").map(|a| a.spelling()),
            Some("U:/Git/x".into())
        );
        assert_eq!(p(r"\\?\UNC\srv\s\x"), None);
        // Equal anchors give one; the order is lexical, then canonical.
        let (l, c) = (std::path::Path::new("/a/ws"), std::path::Path::new("/b/ws"));
        assert_eq!(anchors_of(Some(l), Some(l)).len(), 1);
        assert_eq!(
            anchors_of(Some(l), Some(c))
                .iter()
                .map(Anchor::spelling)
                .collect::<Vec<_>>(),
            ["/a/ws", "/b/ws"]
        );
    }

    /// #363 spec §3 (sol r1 #5): below an anchor a path has ONE reading, the
    /// first matching anchor's, whatever the root; else today's readings.
    #[test]
    fn anchored_path_has_one_reading_the_first_anchors() {
        let root = PathRoot::with_spellings(
            DiffRoot::GitTop("Cloud".into()),
            &["/t/repo".into(), "/t/repo/Cloud".into()],
        );
        assert_eq!(
            path_keys(&root, "/t/repo/Cloud/src/x.al"),
            ["cloud/src/x.al"]
        );
        let swapped = PathRoot::with_spellings(
            DiffRoot::GitTop("Cloud".into()),
            &["/t/repo/Cloud".into(), "/t/repo".into()],
        );
        assert_eq!(path_keys(&swapped, "/t/repo/Cloud/src/x.al"), ["src/x.al"]);
        // A relative path keeps both GitTop readings.
        assert_eq!(
            path_keys(&root, "Cloud/src/x.al"),
            ["cloud/src/x.al", "src/x.al"]
        );
    }

    /// #363 S3: an absolute `Only in` folder that is the workspace or above it
    /// holds its files; one beside it does not.
    #[test]
    fn absolute_only_in_folder_at_or_above_the_workspace() {
        let root = PathRoot::with_spellings(DiffRoot::Workspace, &["/t/repo/Cloud".into()]);
        let folders: std::collections::HashSet<String> = ["src".to_string()].into();
        for (path, holds) in [
            ("/t/repo/Cloud", true),
            ("/t/repo", true),
            ("/", true),
            ("/t/repo/Cloud/src", true),
            ("/t/elsewhere", false),
            ("/t/repo/Cloud/test", false),
        ] {
            assert_eq!(
                holds_workspace_files(&folders, true, &root, path),
                holds,
                "{path}"
            );
        }
        assert!(!holds_workspace_files(&folders, false, &root, "/t/repo"));
    }

    /// #363 spec §4: the wire keeps the root's shape and carries the anchors
    /// beside it; no anchors (an older peer's message) is no anchors.
    #[test]
    fn decision_wire_round_trip() {
        let decided = Some(Ok(PathRoot::with_spellings(
            DiffRoot::GitTop("Cloud".into()),
            &["/t/repo/Cloud".into()],
        )));
        let (root, anchors) = decision_to_wire(&decided);
        assert_eq!(root, Some(Ok(DiffRoot::GitTop("Cloud".into()))));
        assert_eq!(anchors, ["/t/repo/Cloud"]);
        assert_eq!(decision_from_wire(root.clone(), &anchors), decided);
        assert_eq!(
            decision_from_wire(root, &[]),
            Some(Ok(DiffRoot::GitTop("Cloud".into()).into()))
        );
        assert_eq!(
            decision_to_wire(&Some(Err("e".into()))),
            (Some(Err("e".into())), vec![])
        );
    }

    /// #363 spec §4: both wire structs carry `pathAnchors` beside the root,
    /// leave it out when empty, and decode a message without it (an older
    /// peer's) to no anchors.
    #[test]
    fn wire_structs_carry_path_anchors() {
        use crate::engine::batch::JobStreams;
        use crate::engine::batch::worker::WorkerJob;
        use crate::engine::host::Decisions;
        let root = Some(Ok(DiffRoot::GitTop("Cloud".into())));
        let anchors = vec!["/t/repo/Cloud".to_string()];

        let d = Decisions {
            changed_is_file: None,
            diff_root: root.clone(),
            path_anchors: anchors.clone(),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["pathAnchors"], serde_json::json!(anchors));
        assert_eq!(serde_json::from_value::<Decisions>(v.clone()).unwrap(), d);
        let mut legacy = v;
        legacy.as_object_mut().unwrap().remove("pathAnchors");
        let old: Decisions = serde_json::from_value(legacy).unwrap();
        assert!(old.path_anchors.is_empty());
        assert_eq!(old.diff_root, root);
        let empty = Decisions {
            path_anchors: vec![],
            ..d
        };
        assert!(
            serde_json::to_value(&empty)
                .unwrap()
                .get("pathAnchors")
                .is_none()
        );

        let job = WorkerJob {
            argv: vec!["digest".into()],
            streams: JobStreams {
                stdout: "o".into(),
                stderr: "e".into(),
            },
            changed_is_file: None,
            diff_root: root.clone(),
            path_anchors: anchors.clone(),
        };
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v["pathAnchors"], serde_json::json!(anchors));
        let mut legacy = v;
        legacy.as_object_mut().unwrap().remove("pathAnchors");
        let old: WorkerJob = serde_json::from_value(legacy).unwrap();
        assert!(old.path_anchors.is_empty());
        assert_eq!(old.diff_root, root);
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

    /// #362: a `.al` entry makes file inputs before existence is asked, so an
    /// existing `.al` path is never read as diff text. Any part counts, in any
    /// case; the closure panics to prove existence is not consulted.
    #[test]
    fn auto_detect_al_entry_is_files_before_existence() {
        let never = |p: &str| -> bool { panic!("existence asked for {p}") };
        assert_eq!(
            auto_detect_changed_with("Cloud/src/posting.al", never),
            ChangedAutoDetect::Files(vec!["Cloud/src/posting.al".into()])
        );
        assert_eq!(
            auto_detect_changed_with("SRC/X.AL", never),
            ChangedAutoDetect::Files(vec!["SRC/X.AL".into()])
        );
        assert_eq!(
            auto_detect_changed_with("fix.patch, src/a.al", never),
            ChangedAutoDetect::Files(vec!["fix.patch".into(), "src/a.al".into()])
        );
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
            unresolved_literal_dead: 0,
            unresolved_literal_narrowed: 0,
            unresolved_traversal: UnresolvedTraversal {
                truncated: false,
                max_depth: 64,
                visited_routines: 1,
                dependency_routines: 0,
            },
            entry_diagnostics: diagnostics,
            declaration_only: false,
            suppressed_commits: Vec::new(),
            try_root_refusal: false,
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
            flow_notes: None,
            crossing: None,
        }
    }

    /// Round-4 literal routes (c15-7): a crossing's prefix path is taken
    /// only from a row that can run. Hand-stated rows of one crossing: a
    /// dead row with the shorter path and a live row with a longer one; and
    /// the dead row alone, which leaves no prefix.
    ///
    /// DISCRIMINATION (2026-11-08): the `!e.literal_unreachable` condition
    /// in `RoutineRollup::build` removed fails at "the live row's path";
    /// restored, passes.
    #[test]
    fn a_crossing_prefix_comes_from_a_live_row() {
        use crate::engine::l5::digest::{ProjectedHop, QueryWitnessHop};
        let hop = |from: &str, to: &str| ProjectedHop {
            inner: QueryWitnessHop {
                kind: "call",
                from_routine_id: from.to_string(),
                from_display: from.to_string(),
                to_routine_id: Some(to.to_string()),
                to_display: Some(to.to_string()),
                callee_display: None,
                callsite_id: None,
                event_id: None,
                target_app_guid: None,
                edge_kind: Some("direct-call".to_string()),
                anchor: None,
                receiver_type: None,
                interface_name: None,
                candidate_count: None,
                operation_id: None,
            },
        };
        let boundary = [("boundary", "Codeunit Dep::Entry")];
        let dead = || {
            let mut d = effect("UI_CONFIRM", &boundary, &[]);
            d.via_paths = vec![vec![hop("r", "x")]];
            d.literal_unreachable = true;
            d
        };
        let mut live = effect("UI_MESSAGE", &boundary, &[]);
        live.via_paths = vec![vec![hop("r", "a"), hop("a", "b")]];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let mut e = entry_with(vec![]);
        e.effects = vec![dead(), live];
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        assert_eq!(r.crossings.len(), 1, "precondition: one crossing");
        assert_eq!(r.crossings[0].prefix_row, Some(1), "the live row's path");
        let mut e = entry_with(vec![]);
        e.effects = vec![dead()];
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        assert_eq!(r.crossings[0].prefix_row, None, "a dead row gives none");
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
        // Round 2 G10: the totals are in the row's result-index row.
        let row = &s[crate::engine::result_index::ROWS_KEY][0];
        assert_eq!(s["routines"][0]["index"], 1, "{s}");
        assert_eq!(row["effectCounts"]["COMMIT"], 3, "{s}");
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

    /// Round-2 c11: one event entry point into three subscribers is one
    /// entry point, three dependency routines entered and one dependency
    /// call. The `writes beyond dependency calls` header says "dependency
    /// routines entered", so it counts the routines (it printed "1
    /// dependency routine entered" beside the three names). Hand-stated
    /// row: one write group whose crossing has three targets and one site.
    ///
    /// DISCRIMINATION (2026-11-06): counting `points` in the header again
    /// (`counts.writing.points` for `counts.writing.routines` in
    /// `beyond_lines`) fails at "3 dependency routines entered"; restored,
    /// passes.
    #[test]
    fn beyond_header_counts_the_routines_an_entry_point_enters() {
        use crate::engine::l5::digest::{Crossing, CrossingSite, SiteAnchor};
        let opts = crate::engine::l5::routine_rollup::RenderOpts {
            detail: false,
            path_cap: 0,
            surface: crate::engine::follow_up::Surface::Cli,
        };
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let mut e = entry_with(vec![]);
        let mut w = effect(
            "DB_INSERT",
            &[],
            &[("dep/table/1", "T1"), ("dep/table/2", "T2")],
        );
        w.crossing = Some(std::sync::Arc::new(Crossing {
            id: "ev:OnAfterModify@ws:a.al:953".to_string(),
            site: CrossingSite::Event("ev".to_string()),
            edges: Vec::new(),
            targets: vec![
                "dep:A".to_string(),
                "dep:B".to_string(),
                "dep:C".to_string(),
            ],
            sites: vec![SiteAnchor {
                file: Some("ws:a.al".to_string()),
                line: Some(952),
                column: Some(0),
            }],
        }));
        e.effects = vec![w];
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        let lines = crate::engine::l5::routine_rollup::beyond_lines(&r, &e, &cx, opts);
        assert!(
            lines[0].contains("through 3 dependency routines entered at 1 dependency call"),
            "{lines:?}"
        );
        assert_eq!(r.totals.entry_points, 1, "one entry point");
        assert_eq!(r.totals.routines_entered, 3, "three routines entered");
    }

    /// Round-2 n07: "dependency code: none reached" only when the cone walk
    /// reached no dependency routine. A cone that enters dependency code
    /// with no effect beyond it has no entry point (entry points are the
    /// crossings that carry an effect row), and said "none reached".
    /// Hand-stated: an entry with no effect row whose walk reached 2
    /// dependency routines, and the same with 0.
    ///
    /// DISCRIMINATION (2026-11-06): deciding the depth on the entry points
    /// alone (`reached` forced to 0 in `header_facts`) prints "none
    /// reached" for the first entry: fails at "2 dependency routines
    /// reached"; restored, passes.
    #[test]
    fn depth_says_dependency_code_is_reached_without_an_effect() {
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let depth = |reached: usize| {
            let mut e = entry_with(vec![]);
            e.unresolved_traversal.dependency_routines = reached;
            let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
            crate::engine::l5::routine_rollup::header_facts(&r, &e, &[]).depth
        };
        assert_eq!(
            depth(2),
            "dependency code followed through every resolved call (2 dependency routines \
             reached; no effect beyond them)"
        );
        assert_eq!(depth(0), "dependency code: none reached");
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
        // Output validation V4 (§5.2): one compact row per root; round 2
        // G10: its identity and totals are in its result-index row, the
        // listed row names it and gives the short answer.
        let listed = &s["routines"][0];
        assert_eq!(listed["index"], 1, "{s}");
        assert!(
            listed["answer"]
                .as_str()
                .is_some_and(|a| a
                    .starts_with("may write 13 tables (1 owned by this app, 12 by dependencies)")),
            "{s}"
        );
        let r = &s["indexRows"][0];
        assert_eq!(r["routine"], "Codeunit Out Issue Document::IssueDoc");
        // r3 §5.1 (A P10): one map, `effectCounts`.
        assert_eq!(r["effectCounts"]["DB_INSERT"], 2, "{s}");
        assert!(r.get("effectsByType").is_none(), "{s}");
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
        // r3 §5.2: the index row's sentence is the overall effect; the
        // writes-only summary line is `writesAnswer`.
        assert!(
            r["writesAnswer"]
                .as_str()
                .is_some_and(|a| a.starts_with("writes: ")),
            "{s}"
        );
        assert_eq!(r["answer"], listed["answer"], "{s}");
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

    /// A boundary group row of `ty` through a crossing `id` into `targets`,
    /// entered at `site` (`file`, 0-based `line`).
    fn crossing_row(ty: &str, id: &str, targets: &[&str], site: (&str, u32)) -> DigestEffectFull {
        let mut e = effect(ty, &[("boundary", id)], &[]);
        e.crossing = Some(std::sync::Arc::new(crate::engine::l5::digest::Crossing {
            id: id.to_string(),
            site: crate::engine::l5::digest::CrossingSite::Callsite(id.to_string()),
            edges: vec![],
            targets: targets.iter().map(|t| (*t).to_string()).collect(),
            sites: vec![crate::engine::l5::digest::SiteAnchor {
                file: Some(site.0.to_string()),
                line: Some(site.1),
                column: None,
            }],
        }));
        e
    }

    /// Round-4 n10: "dependency routines entered that commit" counts distinct
    /// ROUTINES, never crossings (CDO DocSave printed 3 for `Sales-Calc.
    /// Discount`::OnRun once and `CSC Core`::IsAppActiveOrAskToActivate twice,
    /// which are 2 routines at 3 call sites), and the ruled-out commits count
    /// the same way (they counted COMMIT ROWS: two dead rows of one crossing
    /// were 2). Hand-stated crossings: live COMMIT rows through `cs:a` (into
    /// `dep:Calc`), `cs:b` and `cs:c` (both into `dep:Core`), and `cs:g` (into
    /// `dep:Y`, which has a dead COMMIT row as well, so it is a live entry
    /// only); dead COMMIT rows through `cs:d` (two rows) and `cs:e`, both into
    /// `dep:X`. Expected by hand: live 3 routines at 4 call sites (3 entry
    /// points), ruled out 1 routine at 2 call sites (1 entry point).
    ///
    /// DISCRIMINATION (2026-11-08): counting the routines as crossings
    /// (`crossing_counts` keying every crossing's routine by its own id, not
    /// its targets) fails at "live entries" (routines 4, not 3); counting a
    /// crossing with a live COMMIT row as ruled out too (`&& !c.commit`
    /// removed in `totals_of`) fails at "ruled-out entries" (2 entry points,
    /// not 1); each restored, passes.
    #[test]
    fn commit_entries_count_distinct_routines_not_crossings() {
        use crate::engine::l5::routine_rollup::EntryCount;
        let dead = |id: &str, t: &str, site: (&str, u32)| {
            let mut e = crossing_row("COMMIT", id, &[t], site);
            e.literal_unreachable = true;
            e
        };
        let mut e = entry_with(vec![]);
        e.effects = vec![
            crossing_row("COMMIT", "cs:a", &["dep:Calc"], ("ws:src/F.al", 124)),
            crossing_row("COMMIT", "cs:b", &["dep:Core"], ("ws:src/H.al", 239)),
            crossing_row("COMMIT", "cs:c", &["dep:Core"], ("ws:src/M.al", 479)),
            crossing_row("COMMIT", "cs:g", &["dep:Y"], ("ws:src/G.al", 9)),
            dead("cs:g", "dep:Y", ("ws:src/G.al", 9)),
            dead("cs:d", "dep:X", ("ws:src/D.al", 1)),
            dead("cs:d", "dep:X", ("ws:src/D.al", 1)),
            dead("cs:e", "dep:X", ("ws:src/E.al", 2)),
        ];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        let t = &r.totals;
        assert_eq!(
            t.commit_entries,
            EntryCount {
                points: 3,
                routines: 3,
                sites: 4
            },
            "live entries: {t:?}"
        );
        assert_eq!(
            t.commit_dead_entries,
            EntryCount {
                points: 1,
                routines: 1,
                sites: 2
            },
            "ruled-out entries: {t:?}"
        );
        // The use: the summary's clause, the commits section's header and
        // the JSON state the routines, with the call sites beside them.
        let human = t.human("none");
        assert!(
            human.contains(
                "commits: may (3 dependency routines that commit, entered at 4 call sites; \
                 1 dependency routine entered at 2 call sites more under literal arguments that \
                 cannot run)"
            ),
            "{human}"
        );
        let lines = crate::engine::l5::routine_rollup::commit_lines(
            &r,
            &e,
            &cx,
            crate::engine::l5::routine_rollup::RenderOpts::new(false, None),
        );
        assert_eq!(
            lines[0],
            "commits: may — none in workspace code; 3 dependency routines that commit, entered \
             at 4 call sites:",
            "{lines:?}"
        );
        let v = t.to_json();
        let c = &v["commits"];
        let n = |x: &serde_json::Value, k: &str| x[k].as_u64();
        assert_eq!(
            (n(c, "entryPoints"), n(c, "routines"), n(c, "callSites")),
            (Some(3), Some(3), Some(4)),
            "{c}"
        );
        let d = &c["literalDead"];
        assert_eq!(
            (n(d, "entryPoints"), n(d, "routines"), n(d, "callSites")),
            (Some(1), Some(1), Some(2)),
            "{c}"
        );
    }

    /// Round-4 n10 (bug 1 as reported): "owned by this app" is the table's
    /// DECLARING app, never where the write runs. Hand-stated: workspace code
    /// inserts the dependency tables `dep/table/133` and `dep/table/130` (as
    /// CDO's `CDOElectronicDocumentMgt` inserts Base App `Incoming Document
    /// Attachment`), and a dependency routine entered at a crossing modifies
    /// the workspace table `ws/table/7` (as CDO's `CDO Output Profile
    /// Conflict` is written only beyond `Sales-Calc. Discount`::OnRun).
    /// Expected by hand: 1 owned (`ws/table/7`), 2 by dependencies.
    ///
    /// DISCRIMINATION (2026-11-08): deciding ownership by where the write
    /// runs (`owned` in `totals_of` counted as the workspace-code records,
    /// `self.workspace_code.len()`) fails at "owned by the declaring app";
    /// restored, passes.
    #[test]
    fn owned_tables_are_the_ones_the_workspace_declares() {
        let own_insert = |id: &str, name: &str, line: u32| {
            let mut own = effect(
                "DB_INSERT",
                &[
                    ("resourceId", id),
                    ("resourceDisplay", name),
                    ("tempState", "known(false)"),
                ],
                &[],
            );
            own.evidence.source_kind = "source";
            own.evidence.file = Some("ws:src/A.al".to_string());
            own.evidence.line = Some(line);
            own
        };
        let own = own_insert("dep/table/133", "Incoming Document Attachment", 181);
        let own2 = own_insert("dep/table/130", "Incoming Document", 190);
        let mut beyond = crossing_row("DB_MODIFY", "cs:p", &["dep:Calc"], ("ws:src/F.al", 124));
        beyond.group_resources = vec![("ws/table/7".to_string(), "Conflict".to_string())];
        let mut e = entry_with(vec![]);
        e.effects = vec![own, own2, beyond];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        assert_eq!(r.workspace_code.len(), 2, "two workspace-code records");
        let t = &r.totals;
        assert_eq!(t.may_write_tables, 3, "{t:?}");
        assert_eq!(
            (t.owned_by_app, t.by_dependencies),
            (1, 2),
            "owned by the declaring app: {t:?}"
        );
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

    /// Engine limits wave 0 (B8): crossings into the same dependency routines
    /// print as one item with their places counted, and a folded group hides
    /// all of its places. Hand-stated: eight crossings, each into its own
    /// routine (`dep:B` … `dep:I`), and three into `dep:Z` (sorted last, so
    /// the fold hides it). Expected by hand: 11 places; 9 items, 6 shown;
    /// hidden `dep:H`, `dep:I` and the `dep:Z` group = 1 + 1 + 3 = 5 places.
    /// With `detail` the group lists its three sites.
    ///
    /// DISCRIMINATION (2026-11-09): counting the hidden ITEMS (`hidden` as
    /// `places.len() - shown`, the old `fold` count) prints `+3 places` and
    /// fails at "a folded group hides all its places"; restored, passes.
    #[test]
    fn grouped_crossings_keep_every_place_and_count() {
        use crate::engine::l5::routine_rollup::{RenderOpts, RollupCx, RoutineRollup, kind_lines};
        let mut rows: Vec<DigestEffectFull> = ["B", "C", "D", "E", "F", "G", "H", "I"]
            .iter()
            .enumerate()
            .map(|(i, t)| {
                crossing_row(
                    "BACKGROUND_TASK",
                    &format!("cs:{t}"),
                    &[&format!("dep:{t}")],
                    ("ws:src/A.al", i as u32),
                )
            })
            .collect();
        for (i, id) in ["cs:z1", "cs:z2", "cs:z3"].iter().enumerate() {
            rows.push(crossing_row(
                "BACKGROUND_TASK",
                id,
                &["dep:Z"],
                ("ws:src/Z.al", 10 + i as u32),
            ));
        }
        let mut e = entry_with(vec![]);
        e.effects = rows;
        let model = ws_model();
        let cx = RollupCx::new(&model, &[]);
        let r = RoutineRollup::build(&e, &cx);
        let line = kind_lines(&r, &e, RenderOpts::new(false, None)).join("\n");
        assert!(
            line.starts_with("background tasks at 11 places: "),
            "{line}"
        );
        assert_eq!(
            line.matches("dep:Z").count(),
            0,
            "the group is folded: {line}"
        );
        assert!(
            line.ends_with("; … +5 places (--detail lists them)"),
            "a folded group hides all its places: {line}"
        );
        let all = kind_lines(&r, &e, RenderOpts::new(true, None)).join("\n");
        assert!(
            all.ends_with(
                "BACKGROUND_TASK beyond dep:Z (3 places, entered at ws:src/Z.al lines 11, 12, \
                 13)"
            ),
            "{all}"
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
        // Round 2 G10: the compact rows (their totals in the index, the
        // answer the 40 rows share once in `shared`) all fit; the overflow
        // case is `digest_summary_rows_fit_and_overflow_keeps_the_index`.
        assert_eq!(s["routinesListed"], 40, "{text}");
        assert!(s.get("summaryTruncated").is_none(), "{text}");
        assert_eq!(s["routines"][0]["sharedWith"], "s1", "{text}");
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
