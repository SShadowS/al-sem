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
use crate::engine::l4::effect_query_cli::{OwnerIndex, owner_qualified, temp_state_str};
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::routine_interner::SolveIx;
use crate::engine::l5::conditionality::EffectConditionality;
use crate::engine::l5::diff_parser::{DiffFileKind, HunkChange, parse_unified_diff};
use crate::engine::l5::digest::{
    DigestEntryResult, compute_digest_effects_cli, is_known_temp_state,
};
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, SnapshotCoverageRecord, SnapshotIdentityTable,
};
use crate::engine::l5::transaction_spans::{SeedKind, TransactionSpan};
use crate::engine::l5::unresolved_cone::{
    DEFAULT_MAX_DEPTH, UnresolvedConeItem, UnresolvedTraversal, unresolved_cone,
};
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

/// Strip URI scheme prefix from a snapshot source path.
fn strip_scheme(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("ws:") {
        return rest.replace('\\', "/");
    }
    if let Some(rest) = p.strip_prefix("app:")
        && let Some(i) = rest.find(':')
    {
        return rest[i + 1..].replace('\\', "/");
    }
    p.replace('\\', "/")
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
    SelectorUnmatched {
        selector: String,
    },
    SelectorAmbiguous {
        selector: String,
        candidates: Vec<String>,
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
    DiffParseError {
        /// The file the error belongs to (a hunk-header error, #72); `None`
        /// for a truncated hunk or a header error whose section names no path.
        file: Option<String>,
        detail: String,
    },
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
    /// readings of a diff path name a FILE, routines or not (#77).
    pub workspace_files: &'a [crate::program::inputs::WsFile],
}

/// What a diff's paths are relative to (#77), held as the workspace's path
/// below that directory, `/`-joined with its case kept (`Cloud`, never empty:
/// a root that IS the workspace is [`DiffRoot::Workspace`]).
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

/// [`FileKeys`] of `files`, built once per diff run.
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
    let as_written = || Some(normalize_input(path));
    let readings = match root {
        DiffRoot::Workspace => [as_written(), None],
        DiffRoot::Explicit(p) => [strip_diff_prefix(p, path), None],
        DiffRoot::GitTop(p) => [as_written(), strip_diff_prefix(p, path)],
    };
    readings
        .into_iter()
        .flatten()
        .filter(|k| files.contains_key(k))
        .collect()
}

/// The span-index key a diff path names under `root`, or the diagnostic. One
/// reading: that file, `diff-file-unmatched` when it holds no routine (as
/// before #77). Two: `diff-file-ambiguous` with every unit of both readings.
fn diff_key(
    spans: &RoutineSpanIndex,
    files: &FileKeys<'_>,
    root: &DiffRoot,
    path: &str,
) -> Result<String, ChangedRootsDiagnostic> {
    let readings = diff_readings(files, root, path);
    // A reading's units (`ws:<rel>`), sorted.
    let units = |key: &String| {
        let mut u: Vec<String> = files[key].iter().map(|f| format!("ws:{f}")).collect();
        u.sort();
        u.dedup();
        u
    };
    match &readings[..] {
        [key] if !spans.of_file(key).is_empty() => Ok(key.clone()),
        [_, _] => Err(ChangedRootsDiagnostic::DiffFileAmbiguous {
            file: path.to_string(),
            candidates: readings.iter().flat_map(units).collect(),
        }),
        _ => Err(ChangedRootsDiagnostic::DiffFileUnmatched {
            file: path.to_string(),
        }),
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
    fn touched_by(&self, c: &HunkChange) -> bool {
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

    // 1. File-based matching
    for file in input.files.iter().flatten() {
        let ixs = spans.of_file(&normalize_input(file));
        if ixs.is_empty() {
            diagnostics.push(ChangedRootsDiagnostic::FileUnmatched { file: file.clone() });
        }
        hits.extend_from_slice(ixs);
    }

    // 2. Routine selector matching (port of resolveSelector — 5-form cascade).
    for selector in input.routines.iter().flatten() {
        let (matches, matched_form) = resolve_selector(selector, &selector_idx);
        let matches = narrow_to_owner(selector, matched_form, matches, |id| {
            owners.object_display(id)
        });
        if matches.is_empty() {
            diagnostics.push(ChangedRootsDiagnostic::SelectorUnmatched {
                selector: selector.clone(),
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
            diagnostics.push(ChangedRootsDiagnostic::SelectorAmbiguous {
                selector: selector.clone(),
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
        let files = workspace_file_keys(input.workspace_files);
        let poisoned: std::collections::HashSet<String> = parsed
            .file_errors
            .iter()
            .flat_map(|fe| diff_readings(&files, &input.diff_root, &fe.path))
            .collect();
        for diff_file in &parsed.files {
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
        for file in &parsed.only_in {
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
fn tx_context_map(spans: &[TransactionSpan]) -> HashMap<String, TransactionContext> {
    let mut map: HashMap<String, TransactionContext> = HashMap::new();
    for span in spans {
        if span.seed_kind != SeedKind::ExplicitCommit {
            continue;
        }
        let tc: TransactionContext = if !span.coverage_complete {
            TX_UNKNOWN
        } else if !span.writes_tables.is_empty() {
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
}

pub struct FullAnchor {
    pub source_kind: &'static str, // "source" | "unavailable"
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
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
    fn for_effect(&self, root_stable: &str, op_ids: &[String], effect_is_kt: bool) -> TempJoin {
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
#[derive(Default)]
pub struct DigestEffects {
    pub entries: Vec<DigestEntryResult>,
    pub unresolved: BTreeMap<String, UnresolvedCone>,
    pub coverage: Vec<SnapshotCoverageRecord>,
    pub identities: SnapshotIdentityTable,
    /// `--tests` (#254): the test-coverage graph over the same tree.
    pub coverage_graph: Option<crate::engine::l5::test_coverage::CoverageGraph>,
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
) -> usize {
    let mut count = 0usize;
    for subject in gap_subjects {
        // Direct routine-subject match.
        if visited_ids.contains(*subject) {
            count += 1;
            continue;
        }
        // App-level (symbol-only-boundary) match: subject is a bare app GUID and some
        // visited routine has a stable id of the form `{appGuid}:Type:{n}#hash`.
        let prefix = format!("{}:", subject);
        if visited_ids.iter().any(|v| v.starts_with(&prefix)) {
            count += 1;
        }
    }
    count
}

/// [`DigestEffects`] of `roots` off `tree`: `tests`' coverage graph (#254), the
/// S4 entries and each root's unresolved cone are computed first, then the
/// identity table and the coverage are moved out (the tree's other readers are
/// done with them). `gaps_in_cone`: prove's plan, which counts each cone's
/// analysis gaps. `include_reads`: digest's `--include-reads` (#246).
pub(crate) fn digest_effects(
    tree: &mut CapabilitySnapshot,
    model: &Model,
    roots: &[String],
    tests: bool,
    gaps_in_cone: bool,
    include_reads: bool,
) -> DigestEffects {
    let coverage_graph =
        tests.then(|| crate::engine::l5::test_coverage::CoverageGraph::from_snapshot(tree, model));
    let entries = compute_digest_effects_cli(tree, model, Some(roots), include_reads);
    let gap_subjects: Vec<&str> = if gaps_in_cone {
        tree.analysis_gaps
            .iter()
            .map(|g| g.subject.as_str())
            .collect()
    } else {
        Vec::new()
    };
    let unresolved = roots
        .iter()
        .map(|r| {
            let c = unresolved_cone(tree, r);
            (
                r.clone(),
                UnresolvedCone {
                    gaps_in_cone: gaps_in_cone
                        .then(|| count_gaps_in_cone(&gap_subjects, &c.visited_ids)),
                    items: c.items,
                    traversal: c.traversal,
                },
            )
        })
        .collect();
    DigestEffects {
        entries,
        unresolved,
        coverage: std::mem::take(&mut tree.coverage),
        identities: std::mem::take(&mut tree.identities),
        coverage_graph,
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

    // Coverage map
    let mut coverage_by_id: HashMap<&str, (&str, Vec<&str>)> = HashMap::new();
    for rec in &effects.coverage {
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

                // Evidence anchor
                let evidence = FullAnchor {
                    source_kind: eff.evidence.source_kind,
                    file: eff.evidence.file.clone(),
                    line: eff.evidence.line,
                    column: eff.evidence.column,
                    excerpt: eff.evidence.excerpt.clone(),
                };

                // evidence-unavailable diagnostic
                if eff.evidence.source_kind == "unavailable" {
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
                if matches!(
                    eff.effect_type.as_str(),
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" | "DB_READ"
                ) {
                    let join = temp_states.for_effect(
                        rid,
                        &eff.origin_operation_ids,
                        is_known_temp_state(eff.temp_state.as_ref()),
                    );
                    if let Some(ts) = join.render() {
                        detail.push(("tempState".to_string(), ts));
                    }
                    entry_diagnostics.extend(incoherence_diagnostics(
                        &eff.effect_type,
                        &eff.fact_subject,
                        &eff.fact_id,
                        join.problems,
                    ));
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
                    group_resources: std::mem::take(&mut eff.group_resources),
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

pub fn anchor_to_value(a: &FullAnchor) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("sourceKind".into(), a.source_kind.into());
    if let Some(ref f) = a.file {
        m.insert("file".into(), f.clone().into());
    }
    if let Some(l) = a.line {
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

fn scoped_guarantee_to_value(
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

pub fn effect_to_value(eff: &DigestEffectFull) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), eff.effect_type.clone().into());

    // detail: insertion-order map
    let mut detail_obj = serde_json::Map::new();
    for (k, v) in &eff.detail {
        detail_obj.insert(k.clone(), v.clone().into());
    }
    m.insert("detail".into(), serde_json::Value::Object(detail_obj));

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
    m.insert("confidence".into(), "static".into());
    m.insert("conditionality".into(), eff.conditionality.into());
    m.insert("transactionContext".into(), eff.transaction_context.into());
    let guarantees: Vec<serde_json::Value> =
        eff.guarantees.iter().map(|g| g.clone().into()).collect();
    m.insert("guarantees".into(), serde_json::Value::Array(guarantees));
    m.insert("factId".into(), eff.fact_id.clone().into());
    let sg: Vec<serde_json::Value> = eff
        .scoped_guarantees
        .iter()
        .map(scoped_guarantee_to_value)
        .collect();
    m.insert("scopedGuarantees".into(), serde_json::Value::Array(sg));

    serde_json::Value::Object(m)
}

fn unresolved_item_to_value(item: &UnresolvedConeItem) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(ref f) = item.callsite_file {
        // Build callsiteAnchor
        let mut anch = serde_json::Map::new();
        anch.insert("sourceKind".into(), "source".into());
        anch.insert("file".into(), f.clone().into());
        if let Some(l) = item.callsite_line {
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

    // entryDiagnostics
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
    m.insert("entryDiagnostics".into(), serde_json::Value::Array(ed));

    // routine
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
    m.insert("routine".into(), serde_json::Value::Object(routine_m));

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

fn changed_roots_diagnostic_to_value(d: &ChangedRootsDiagnostic) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    match d {
        ChangedRootsDiagnostic::FileUnmatched { file } => {
            m.insert("kind".into(), "file-unmatched".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::SelectorUnmatched { selector } => {
            m.insert("kind".into(), "selector-unmatched".into());
            m.insert("selector".into(), selector.clone().into());
        }
        ChangedRootsDiagnostic::SelectorAmbiguous {
            selector,
            candidates,
        } => {
            m.insert("kind".into(), "selector-ambiguous".into());
            m.insert("selector".into(), selector.clone().into());
            let cv: Vec<serde_json::Value> = candidates.iter().map(|c| c.clone().into()).collect();
            m.insert("candidates".into(), serde_json::Value::Array(cv));
        }
        ChangedRootsDiagnostic::DiffFileUnmatched { file } => {
            m.insert("kind".into(), "diff-file-unmatched".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::DiffFileAmbiguous { file, candidates } => {
            m.insert("kind".into(), "diff-file-ambiguous".into());
            m.insert("file".into(), file.clone().into());
            m.insert("candidates".into(), candidates.clone().into());
        }
        ChangedRootsDiagnostic::HunkOutsideRoutines {
            file,
            start_line,
            end_line,
        } => {
            m.insert("kind".into(), "hunk-outside-routines".into());
            m.insert("file".into(), file.clone().into());
            m.insert("startLine".into(), (*start_line as u64).into());
            m.insert("endLine".into(), (*end_line as u64).into());
        }
        ChangedRootsDiagnostic::DiffFileDeleted { file } => {
            m.insert("kind".into(), "diff-file-deleted".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::DiffParseError { file, detail } => {
            m.insert("kind".into(), "diff-parse-error".into());
            if let Some(file) = file {
                m.insert("file".into(), file.clone().into());
            }
            m.insert("detail".into(), detail.clone().into());
        }
    }
    serde_json::Value::Object(m)
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

    // entries
    let entries_val: Vec<serde_json::Value> = entries.iter().map(entry_to_value).collect();

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
    payload.insert("summary".into(), serde_json::Value::Object(summary));
    payload.insert(
        "workspaceFingerprint".into(),
        workspace_fp.to_string().into(),
    );

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
    // with `file` and `candidates` (#77). Contract: a minor bump is additive;
    // `changedRootsDiagnostics[].kind` is an open set (a consumer treats an
    // unknown kind as a diagnostic, not a parse error); exit 0 does not mean
    // every diff file resolved.
    env.insert("schemaVersion".into(), "1.7.0".into());

    // The block goes in after the null drop, so its `ratio: null` survives; the
    // keep-nulls sort then orders it (and the payload key it lands in).
    let mut doc = sort_and_drop_nulls(serde_json::Value::Object(env));
    if let Some(block) = test_coverage {
        doc["payload"]["testCoverage"] = block;
        doc = sort_keys_keep_nulls(doc);
    }
    serialize_normalized_value(&doc)
}

pub struct ChangedInputContract {
    pub files: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub diff_provided: bool,
}

// ---------------------------------------------------------------------------
// Human formatter (formatDigestHuman)
// ---------------------------------------------------------------------------

pub fn format_digest_human(entries: &[DigestEntryFull]) -> String {
    if entries.is_empty() {
        return "digest: no changed roots resolved\n".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for entry in entries {
        let display = owner_qualified(&entry.object_display, &entry.routine_display);
        lines.push(format!("--- {} ---", display));
        lines.push(format!("  coverage: {}", entry.coverage_status));
        if !entry.coverage_reasons.is_empty() {
            lines.push(format!("  reasons: {}", entry.coverage_reasons.join(", ")));
        }
        if entry.effects.is_empty() {
            lines.push("  (no effects)".to_string());
        }
        for eff in &entry.effects {
            let prov_tag = if eff.provenance == "direct" {
                "direct"
            } else {
                "transitive"
            };
            let ev_file = eff.evidence.file.as_deref().unwrap_or("(unavailable)");
            let ev_line = eff
                .evidence
                .line
                .map(|l| format!(":{}", l))
                .unwrap_or_default();
            let excerpt = eff
                .evidence
                .excerpt
                .as_deref()
                .map(|e| format!(" \"{}\"", e))
                .unwrap_or_default();
            lines.push(format!("  [{}] {}", eff.effect_type, prov_tag));
            lines.push(format!("    evidence: {}{}{}", ev_file, ev_line, excerpt));
            let detail_entries: Vec<String> = eff
                .detail
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            if !detail_entries.is_empty() {
                lines.push(format!("    detail: {}", detail_entries.join(", ")));
            }
            if !eff.via_paths.is_empty() {
                let truncated_str = if eff.via_paths_truncated {
                    " (truncated)"
                } else {
                    ""
                };
                lines.push(format!(
                    "    via: {} path(s){}",
                    eff.via_paths.len(),
                    truncated_str
                ));
            }
        }
        if !entry.unresolved.is_empty() {
            lines.push(format!(
                "  unresolved: {} callsite(s)",
                entry.unresolved.len()
            ));
            if entry.unresolved_traversal.truncated {
                lines.push("    (traversal truncated)".to_string());
            }
        }
        if !entry.entry_diagnostics.is_empty() {
            for d in &entry.entry_diagnostics {
                let et_part = d
                    .effect_type
                    .as_deref()
                    .map(|et| format!(" ({})", et))
                    .unwrap_or_default();
                lines.push(format!(
                    "  diagnostic: {}{}{}{}",
                    d.kind,
                    et_part,
                    d.reason.map(|r| format!(" reason={r}")).unwrap_or_default(),
                    d.operation_id
                        .as_deref()
                        .map(|o| format!(" op={o}"))
                        .unwrap_or_default()
                ));
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
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
    /// What `diff_text`'s paths are relative to (#77).
    pub diff_root: DiffRoot,
}

/// [`run_digest_pipeline`] over a given detector set (#160 §4.4.4): the plan
/// is built from `detectors` by `digest_plan`, so a set without
/// `TRANSACTION_SPANS` makes `prepare` panic (A2.10).
pub(crate) fn digest_pipeline_with(
    detectors: Vec<crate::engine::l5::registry::Detector>,
    req: DigestRequest<'_>,
) -> Result<DigestRunResult, String> {
    let job = DigestJob {
        detectors,
        req,
        human: false,
        out: None,
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
}

impl<'a> DigestJob<'a> {
    /// The default detectors over `req`, written as `--format` and `--out` say.
    pub fn new(req: DigestRequest<'a>, human: bool, out: Option<&'a str>) -> DigestJob<'a> {
        DigestJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            req,
            human,
            out,
        }
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
        Ok(PlanPart {
            plan,
            reads: vec![Cell::Det, Cell::L4, Cell::Effects],
            carry: Some(Box::new(changed_roots)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Ok::<_, String>(digest_view(cx, &self.req))))
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
            Ok(r) => JobOutput::text(
                self.out,
                r.map(|r| {
                    let text = if self.human {
                        r.human_text
                    } else {
                        r.json_text
                    };
                    (text, r.exit_code)
                }),
            ),
        }
    }
}

/// Digest off its prepared view: the default run (the envelope diagnostics),
/// the tx map off its `det`, the effects off its tree and the L4 store off its
/// `l4` (#160 §4.4.4, step 6 D7).
fn digest_view(cx: &mut ViewCx<'_>, req: &DigestRequest<'_>) -> DigestRunResult {
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
    let query_diagnostics = query_full.query_diagnostics;
    // #254: the coverage block reads the graph the effects step built off the
    // tree (#160 step 6: the tree itself is released inside `prepare`). A
    // `--tests` plan always builds the tree, roots or not (`digest_plan`).
    let test_coverage = tests.then(|| {
        let changed = coverage_changed_roots(&changed_roots.roots, &entries, &owners);
        crate::engine::l5::test_coverage::compute_test_coverage(
            effects
                .coverage_graph
                .as_ref()
                .expect("a --tests plan builds the coverage graph"),
            &changed,
        )
    });
    // #69 D1.4: the query's inputs (the effects, the L4 store) are dead here.
    // Free them before the JSON projection.
    drop(temp_states);
    drop(tx_ctx_map);
    drop(owners);
    drop(effects);
    cx.release(Cell::L4);
    cx.release(Cell::Effects);

    let exit_code: u8 = if changed_roots.roots.is_empty() { 2 } else { 0 };

    let changed_input_contract = ChangedInputContract {
        files: changed_files.clone(),
        routines: changed_routines.clone(),
        diff_provided: diff_text.is_some(),
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
    );

    // Human. The dependencies line appears only when apps were loaded, so a
    // workspace with no dependency reads the same in both modes.
    let mut human_text = dependencies.prefix_human(format_digest_human(&entries));
    if let Some(t) = &test_coverage {
        human_text.push_str(&t.human_line());
        human_text.push('\n');
    }

    DigestRunResult {
        json_text,
        human_text,
        exit_code,
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
        let text = format_digest_human(&[entry_with(diags)]);
        assert!(
            text.contains(
                "diagnostic: temp-state-incoherent (DB_INSERT) reason=class-absent op=op-a"
            ),
            "reason=class-absent op=op-a missing: {text}"
        );
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
        let count = count_gaps_in_cone(&gap_subjects, &visited);
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
        let count = count_gaps_in_cone(&gap_subjects, &visited);
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
        let count = count_gaps_in_cone(&gap_subjects, &visited);
        assert_eq!(count, 0, "unrelated gap subject must not be counted");
    }
}
