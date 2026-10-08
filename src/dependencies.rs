//! Dependency resolution for AL projects
//!
//! Parses app.json to discover dependencies and locates matching .app files
//! in the .alpackages folder.

use crate::app_package::{ParsedAppPackage, extract_app_package};
use crate::program::dep_cache::DepCache;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// A dependency declared in app.json
#[derive(Debug, Clone, Deserialize)]
pub struct AppDependency {
    /// The dependency's stable GUID. From app.json's `id` field, or the
    /// NavxManifest `Dependency@Id` — the only globally-unique identity.
    #[serde(rename = "id", default)]
    pub app_id: String,
    pub name: String,
    #[serde(default)]
    pub publisher: String,
    pub version: String,
}

/// Parsed app.json structure (only the fields we care about)
#[derive(Debug, Deserialize)]
struct AppJson {
    #[serde(default)]
    dependencies: Vec<AppDependency>,
}

/// A resolved dependency with its parsed package
#[derive(Debug)]
// `dependency` carried for future consumers / Debug.
#[allow(dead_code)]
pub struct ResolvedDependency {
    pub dependency: AppDependency,
    pub app_path: PathBuf,
    pub package: std::sync::Arc<ParsedAppPackage>,
    /// `app_path`'s stamp, taken BEFORE any of its bytes were read.
    pub stamp: Option<AppFileStamp>,
}

/// An `.app` file's size and modification time, taken BEFORE its bytes are
/// read. Keys the shared dependency tier (`program::dep_cache::DepKey`): a
/// file replaced after the stamp but before the read is cached under the OLD
/// stamp, so the next rebuild (which stamps the new file) misses and rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppFileStamp {
    pub len: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl AppFileStamp {
    /// `None` when the file cannot be stat'ed.
    pub fn of(path: &Path) -> Option<AppFileStamp> {
        let meta = std::fs::metadata(path).ok()?;
        Some(AppFileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// One `.app` file dropped during GUID-level dependency dedup (Tier-1
/// remediation, H-2). Two or more entries discovered under `.alpackages`
/// shared a real GUID — a stale version cached in an ancestor folder
/// alongside the correct one, or the SAME version physically present twice
/// (a file copied into two scanned folders) — so only the highest-version
/// (or, for a byte-identical tie, first-encountered) survivor is kept. This
/// names the loser so the drop is never silent; see
/// [`load_all_apps`]'s doc for the defect this closes.
#[derive(Debug, Clone)]
pub struct DroppedDuplicateDependency {
    pub guid: String,
    pub name: String,
    pub kept_version: String,
    pub kept_path: PathBuf,
    pub dropped_version: String,
    pub dropped_path: PathBuf,
}

/// A dependency `.app` the loader could not use (engine-switch S5.2): its
/// manifest could not be read (identity unknown), or every copy of its GUID
/// failed symbol extraction. It does not become an app unit, so without this
/// record it would look exactly like an app that was never there.
#[derive(Debug, Clone)]
pub struct UnreadableDependency {
    pub path: PathBuf,
    /// `None` when the manifest itself could not be read.
    pub guid: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub error: String,
}

/// What [`load_all_apps`] set aside rather than loaded.
#[derive(Debug, Clone, Default)]
pub struct DependencyLoadReport {
    pub dropped: Vec<DroppedDuplicateDependency>,
    pub unreadable: Vec<UnreadableDependency>,
}

/// One `.app` discovered on disk with ONLY its manifest read — the
/// pre-symbol-extraction identity `load_all_apps` dedups on
/// (perf safe-wins Task 3).
#[derive(Debug)]
struct DiscoveredApp {
    app_path: PathBuf,
    meta: crate::app_package::AppMetadata,
    /// Taken before the manifest read — see [`AppFileStamp`].
    stamp: Option<AppFileStamp>,
}

/// Collapse `deps` down to one entry per non-empty GUID, keeping the
/// highest-version survivor (Tier-1 remediation, H-2 root cause).
///
/// Before this, EVERY physically-discovered `.app` became its own graph-level
/// app unit — including a stale version cached in an ancestor `.alpackages`
/// (`find_all_alpackages_folders` walks ancestors on purpose) or the
/// IDENTICAL file present under two scanned folders. Two consequences: (1)
/// `program::build::build_program_graph`'s dependency-closure GUID-match
/// (`by_guid.find(...)`, picking the FIRST match in whatever order the caller
/// iterates) silently bound to whichever version sorted first — determined by
/// the naive lexicographic-string sort below, not version magnitude
/// ("stale-wins"); (2) a byte-identical duplicate ingests TWICE, producing
/// IDENTICAL `RoutineNodeId`s for every one of that app's ABI routines —
/// `dedup_routines_preserving_genuine_overloads` then marks EVERY survivor
/// `abi_overload_collapsed` (a same-key run of >=2 raw entries is
/// indistinguishable from a genuine fingerprint collision), so the entire
/// app's routines decline for chain-typing and plain-dispatch alike
/// ("collapse-poisoning").
///
/// GUID-less entries (a malformed/legacy manifest) are never deduped against
/// each other — without a real identity, collapsing two coincidentally
/// similar entries risks silently merging two GENUINELY different apps,
/// which is worse than leaving both (fail-closed: only collapse what we can
/// prove is the same app).
///
/// Ties (identical version under the same GUID — the true byte-identical-
/// duplicate case) keep the entry with the lexicographically-first path
/// (arbitrary but deterministic, independent of filesystem iteration order —
/// same "arbitrary but stable" convention as
/// `program::build::dedup_routines_preserving_genuine_overloads`).
///
/// Perf safe-wins Task 3: identity now comes straight from the manifest
/// ([`DiscoveredApp::meta`]) rather than from a fully-extracted
/// `ResolvedDependency` — the same values the old code read from
/// `package.metadata` (itself built from the manifest, verbatim), just read
/// one phase earlier, BEFORE any SymbolReference.json is parsed. A loser is
/// now dropped without its symbol blob ever being touched.
///
/// Kept as a thin `#[cfg(test)]`-only wrapper over [`group_discovered_by_guid`]
/// so the five scenario tests below stay exercising the exact "keep index 0,
/// drop the rest" policy in isolation. `load_all_apps` itself calls
/// `group_discovered_by_guid` directly — it needs the FULL ordered candidate
/// list per GUID (not just the winner) for the corrupt-winner fallback (see
/// that function's doc).
#[cfg(test)]
fn dedup_by_guid_keep_highest_version(
    deps: Vec<DiscoveredApp>,
) -> (Vec<DiscoveredApp>, Vec<DroppedDuplicateDependency>) {
    let mut kept: Vec<DiscoveredApp> = Vec::new();
    let mut dropped: Vec<DroppedDuplicateDependency> = Vec::new();

    for mut group in group_discovered_by_guid(deps) {
        if group.len() == 1 {
            kept.push(group.pop().expect("len == 1"));
            continue;
        }
        let mut iter = group.into_iter();
        let winner = iter.next().expect("group.len() > 1");
        for loser in iter {
            dropped.push(DroppedDuplicateDependency {
                guid: winner.meta.app_id.clone(),
                name: winner.meta.name.clone(),
                kept_version: winner.meta.version.clone(),
                kept_path: winner.app_path.clone(),
                dropped_version: loser.meta.version.clone(),
                dropped_path: loser.app_path.clone(),
            });
        }
        kept.push(winner);
    }

    (kept, dropped)
}

/// Group `deps` into one bucket per non-empty GUID (highest version first,
/// ties broken by path for determinism — same policy
/// [`dedup_by_guid_keep_highest_version`]'s doc describes), plus one
/// singleton bucket per GUID-less entry (never merged with anything else —
/// see that function's doc for why). Shared by
/// [`dedup_by_guid_keep_highest_version`] (the simple "keep index 0" policy)
/// and `load_all_apps`'s Phase 3 (which needs the FULL ordered candidate
/// list per GUID so a winner whose symbols turn out to be corrupt can fall
/// back to the next-best copy instead of the dependency vanishing).
fn group_discovered_by_guid(deps: Vec<DiscoveredApp>) -> Vec<Vec<DiscoveredApp>> {
    let mut by_guid: std::collections::HashMap<String, Vec<DiscoveredApp>> =
        std::collections::HashMap::new();
    let mut groups: Vec<Vec<DiscoveredApp>> = Vec::new();

    for rd in deps {
        if rd.meta.app_id.is_empty() {
            groups.push(vec![rd]);
        } else {
            by_guid.entry(rd.meta.app_id.clone()).or_default().push(rd);
        }
    }

    for (_guid, mut group) in by_guid {
        // Highest version first (`compare_versions`: higher version sorts
        // first — see its doc); ties broken by path for determinism.
        group.sort_by(|a, b| {
            compare_versions(&a.meta.version, &b.meta.version)
                .then_with(|| a.app_path.cmp(&b.app_path))
        });
        groups.push(group);
    }

    groups
}

/// Parse app.json to extract dependencies
pub fn parse_app_json(path: &Path) -> Result<Vec<AppDependency>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let app_json: AppJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    Ok(app_json.dependencies)
}

/// Well-known Microsoft Application-tier app identities. BC apps never list these in
/// `dependencies[]` — they are implicit, declared via the `application` field in
/// app.json. Mirrors al-sem `MS_APPLICATION_TIER` in `workspace-dependencies.ts`
/// (same GUIDs, names, publisher, ORDER).
///
/// The single source of truth for the tier data (moved here from
/// `engine::deps::cross_app_l3` in engine-switch S1, so the loader no longer
/// imports the legacy engine): [`append_implicit_ms_tier_deps`] and the legacy
/// cross-app builder both read it.
pub(crate) const MS_APPLICATION_TIER: &[(&str, &str)] = &[
    ("c1335042-3002-4257-bf8a-75c898ccb1b8", "Application"),
    ("437dbf0e-84ff-417a-965d-ed2bb9650972", "Base Application"),
    (
        "f3552374-a1f2-4356-848e-196002525837",
        "Business Foundation",
    ),
];

/// Well-known Microsoft Platform-tier app identities. BC apps never list these in
/// `dependencies[]` — they are implicit, declared via the `platform` field in
/// app.json. Mirrors al-sem `MS_PLATFORM_TIER` in `workspace-dependencies.ts`
/// (same GUIDs, names, publisher, ORDER). See [`MS_APPLICATION_TIER`].
pub(crate) const MS_PLATFORM_TIER: &[(&str, &str)] = &[
    ("63ca2fa4-4f03-4f2b-a480-172fef340d3f", "System Application"),
    ("8874ed3a-0643-4247-9ced-7a7002f7135d", "System"),
];

/// Append IMPLICIT Microsoft Application-/Platform-tier dependency rows to an
/// app's `declared_deps` (beyond-1B.3b Task 5.5 — THE dominant lever for the
/// real-`unknown` burndown).
///
/// Real BC apps declare Base App / System App via the top-level `application`/
/// `platform` VERSION STRING fields in app.json (or the NavxManifest
/// `App@Application`/`App@Platform` attributes for a dependency `.app`) — NEVER
/// via `dependencies[]`. Before this, `src/program`'s closure builder read only
/// the explicit `dependencies[]` array, so Base App/System App were
/// systematically absent from every app's dependency closure and every
/// cross-Microsoft-layer call resolved `OutOfClosure` → `Unknown`.
///
/// Mirrors al-sem `parseWorkspaceDependencies` (the template was
/// `engine::deps::cross_app_l3::read_workspace_declared_dependencies`, deleted with
/// L3 in engine-switch S9.6):
/// a non-empty `application` appends [`MS_APPLICATION_TIER`]
/// (using the `application` string as each row's `version`); a non-empty
/// `platform` appends [`MS_PLATFORM_TIER`]
/// likewise. An empty/absent field injects NOTHING — fixtures with a minimal
/// app.json (no `application`/`platform`) stay unaffected (low ripple).
///
/// `own_guid` guards against self-referential injection: an app never
/// implicitly depends on itself (e.g. if Base App's own manifest carried a
/// non-empty `application`, it must not gain itself as a "dependency" — the
/// `DependencyGraph::closure` DFS is cycle-safe regardless, but a self-edge is
/// still meaningless topology noise). Tier entries whose GUID matches
/// `own_guid` are skipped.
pub fn append_implicit_ms_tier_deps(
    declared: &mut Vec<AppDependency>,
    own_guid: &str,
    application: Option<&str>,
    platform: Option<&str>,
) {
    if let Some(app_ver) = application.filter(|s| !s.is_empty()) {
        for (guid, name) in MS_APPLICATION_TIER {
            if *guid == own_guid {
                continue;
            }
            declared.push(AppDependency {
                app_id: guid.to_string(),
                name: name.to_string(),
                publisher: "Microsoft".to_string(),
                version: app_ver.to_string(),
            });
        }
    }

    if let Some(plat_ver) = platform.filter(|s| !s.is_empty()) {
        for (guid, name) in MS_PLATFORM_TIER {
            if *guid == own_guid {
                continue;
            }
            declared.push(AppDependency {
                app_id: guid.to_string(),
                name: name.to_string(),
                publisher: "Microsoft".to_string(),
                version: plat_ver.to_string(),
            });
        }
    }
}

/// Find the .alpackages folder for a project.
pub fn find_alpackages_folder(project_root: &Path) -> Option<PathBuf> {
    let alpackages = project_root.join(".alpackages");
    if alpackages.is_dir() {
        Some(alpackages)
    } else {
        None
    }
}

/// Discover every `.alpackages` folder reachable from `project_root` by
/// walking up the directory tree. The first entry is always the project's
/// own `.alpackages` (or absent if not present). Subsequent entries are
/// ancestor folders' `.alpackages` directories.
///
/// Walking stops at the first of:
///   - filesystem root,
///   - eight ancestors deep (safety),
///   - a directory containing `.git` (project boundary).
///
/// This mirrors the Go wrapper's `DiscoverPackageCachePaths` so the
/// dependency index that the wrapper relies on (for hover enrichment +
/// dependencyDocumentSymbol RPC) matches what AL LSP itself sees via
/// the augmented `packageCachePaths`. Without this, monorepos that keep
/// shared .app files in an ancestor folder would expose those files to
/// AL LSP but not to al-call-hierarchy, leading to inconsistent symbol
/// data between the two indexes.
pub fn find_all_alpackages_folders(project_root: &Path) -> Vec<PathBuf> {
    let mut folders = Vec::new();

    if let Some(own) = find_alpackages_folder(project_root) {
        folders.push(own);
    }

    let mut current = project_root.parent();
    let mut depth = 0;
    while let Some(dir) = current {
        if depth >= 8 {
            break;
        }

        let alpkg = dir.join(".alpackages");
        if alpkg.is_dir() && !folders.iter().any(|p| paths_equal(p, &alpkg)) {
            folders.push(alpkg);
        }

        if dir.join(".git").exists() {
            break;
        }

        current = dir.parent();
        depth += 1;
    }

    folders
}

/// One `.app` file the dependency loader will consider, and the `.alpackages`
/// folder it was found in.
#[derive(Debug, Clone)]
pub struct DiscoveredAppFile {
    pub folder: PathBuf,
    pub path: PathBuf,
}

/// Every `.app` file [`load_all_apps`] considers for `project_root`: each
/// folder of [`find_all_alpackages_folders`] in walk order, `.app` files only,
/// the same file reached twice (symlinks) kept once by canonical path. The
/// second value lists folders whose listing failed.
///
/// The ONE discovery routine: `load_all_apps` and the CDO baseline's dependency
/// closure digest both read it, so the digest pins exactly the files the
/// resolver loads (#29: it used to hash only `<root>/.alpackages`, while the
/// loader also reads every ancestor cache).
pub fn discover_app_files(
    project_root: &Path,
) -> (Vec<DiscoveredAppFile>, Vec<(PathBuf, std::io::Error)>) {
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    let mut seen_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for folder in find_all_alpackages_folders(project_root) {
        let entries = match std::fs::read_dir(&folder) {
            Ok(e) => e,
            Err(e) => {
                unreadable.push((folder, e));
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("app") {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen_paths.insert(canonical) {
                continue;
            }
            files.push(DiscoveredAppFile {
                folder: folder.clone(),
                path,
            });
        }
    }
    (files, unreadable)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a2), Ok(b2)) => a2 == b2,
        _ => a == b,
    }
}

/// Parse a version string into comparable parts
/// Handles versions like "26.0.0.0" or "26.0.30643.32100"
fn parse_version(version: &str) -> Vec<u64> {
    version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

/// Check if actual version is compatible with required version
/// A version is compatible if it's >= the required version in major.minor
fn is_version_compatible(required: &str, actual: &str) -> bool {
    let req_parts = parse_version(required);
    let actual_parts = parse_version(actual);

    // Compare major.minor at minimum
    for i in 0..2.min(req_parts.len()) {
        let req = req_parts.get(i).copied().unwrap_or(0);
        let act = actual_parts.get(i).copied().unwrap_or(0);

        if act > req {
            return true;
        }
        if act < req {
            return false;
        }
    }

    // Major.minor are equal, so it's compatible
    true
}

/// Compare two version strings for sorting (higher version comes first)
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts = parse_version(a);
    let b_parts = parse_version(b);

    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let a_part = a_parts.get(i).copied().unwrap_or(0);
        let b_part = b_parts.get(i).copied().unwrap_or(0);

        match b_part.cmp(&a_part) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    std::cmp::Ordering::Equal
}

/// Find a matching .app file for a dependency
/// Returns the path to the best matching .app file (highest compatible version)
pub fn find_matching_app(alpackages: &Path, dep: &AppDependency) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(alpackages) {
        Ok(e) => e,
        Err(_) => return None,
    };

    // Normalize publisher and name for matching
    // File names use underscores: "Publisher_Name_Version.app"
    let expected_prefix = format!("{}_{}_", dep.publisher, dep.name);

    let mut candidates: Vec<(PathBuf, String)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "app").unwrap_or(false)
            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
            && filename.starts_with(&expected_prefix)
        {
            // Extract version from filename
            // Format: Publisher_Name_Version.app
            let version_part = &filename[expected_prefix.len()..];
            if let Some(version) = version_part.strip_suffix(".app")
                && is_version_compatible(&dep.version, version)
            {
                candidates.push((path.clone(), version.to_string()));
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Sort by version descending (highest first)
    candidates.sort_by(|a, b| compare_versions(&a.1, &b.1));

    // Return the highest compatible version
    candidates.into_iter().next().map(|(path, _)| path)
}

/// Load every `.app` file present in a project's `.alpackages` folder.
///
/// Unlike `resolve_all`, this doesn't filter by `app.json` declarations —
/// every package found is parsed. Lets us index transitive dependencies
/// (e.g. Base Application) that are sitting in `.alpackages` but aren't
/// listed in the project's direct dependency tree. Mirrors AL LSP behavior.
///
/// Results are deduped at the GUID level (Tier-1 remediation, H-2 — see
/// [`dedup_by_guid_keep_highest_version`]'s doc for the defect this closes)
/// BEFORE the caller ever sees them, so every downstream consumer (this
/// function has more than one caller) is protected uniformly. The second
/// return value names every dropped duplicate and every unreadable package;
/// callers that don't need it can ignore it, but it is never silently
/// discarded here.
pub fn load_all_apps(
    project_root: &Path,
) -> Result<(Vec<ResolvedDependency>, DependencyLoadReport)> {
    load_all_apps_with(project_root, &DepCache::default())
}

/// [`load_all_apps`] sharing each parsed package through `cache`: roots that
/// load the same `.app` hold one copy of its symbols.
pub fn load_all_apps_with(
    project_root: &Path,
    cache: &DepCache,
) -> Result<(Vec<ResolvedDependency>, DependencyLoadReport)> {
    let (files, unreadable) = discover_app_files(project_root);
    for (folder, e) in &unreadable {
        warn!(
            "load_all_apps: read_dir({}) failed: {}",
            folder.display(),
            e
        );
    }
    if files.is_empty() && unreadable.is_empty() {
        debug!(
            "load_all_apps: no .app under any .alpackages at {} or its ancestors",
            project_root.display()
        );
        return Ok((Vec::new(), DependencyLoadReport::default()));
    }

    let mut discovered: Vec<DiscoveredApp> = Vec::new();
    let mut unreadable_apps: Vec<UnreadableDependency> = Vec::new();

    for DiscoveredAppFile {
        folder: alpackages,
        path,
    } in files
    {
        // Stamp BEFORE the first read of this file's bytes (see `AppFileStamp`).
        let stamp = AppFileStamp::of(&path);
        // Phase 1: manifest-only discovery — never touches SymbolReference.json.
        match crate::app_package::extract_app_metadata(&path) {
            Ok(meta) => {
                debug!(
                    "load_all_apps: discovered {} v{} from {}",
                    meta.name,
                    meta.version,
                    alpackages.display()
                );
                discovered.push(DiscoveredApp {
                    app_path: path,
                    meta,
                    stamp,
                });
            }
            Err(e) => {
                warn!("{}", manifest_read_warning(&path, &e));
                unreadable_apps.push(UnreadableDependency {
                    path,
                    guid: None,
                    name: None,
                    version: None,
                    error: format!("{e:#}"),
                });
            }
        }
    }

    // Phase 2: GUID grouping on manifest identity (H-2) — each group is
    // ordered highest-version-first (ties broken by path), NOT yet committed
    // to a winner. That commitment happens in Phase 3 below, because a
    // group's best candidate can still fail symbol extraction (a corrupt
    // SymbolReference.json) — see `group_discovered_by_guid`'s doc.
    let groups = group_discovered_by_guid(discovered);

    // Phase 3: full symbol extraction, trying each group's candidates
    // best-first until one succeeds (availability regression fix: a
    // corrupt-symbols dedup winner must fall back to the next-highest good
    // copy of the SAME GUID rather than the dependency vanishing entirely —
    // the old symbols-first code effectively got this for free by trying
    // every physically-discovered copy before dedup ever ran). Candidates
    // that fail before a winner is found are warned about but NOT reported
    // as dedup drops (they never "lost" a dedup decision — they were simply
    // unreadable); only candidates ranked BELOW the eventual winner are
    // genuine dedup drops.
    let mut out: Vec<ResolvedDependency> = Vec::new();
    let mut dropped: Vec<DroppedDuplicateDependency> = Vec::new();
    for group in groups {
        let mut winner_idx: Option<usize> = None;
        let mut first_error: Option<String> = None;
        for (i, candidate) in group.iter().enumerate() {
            let loaded = cache.package(&candidate.app_path, candidate.stamp, || {
                crate::app_package::extract_app_symbols(&candidate.app_path).map(|objects| {
                    ParsedAppPackage {
                        metadata: candidate.meta.clone(),
                        objects,
                    }
                })
            });
            match loaded {
                Ok(package) => {
                    debug!(
                        "load_all_apps: loaded {} v{} ({} objects)",
                        candidate.meta.name,
                        candidate.meta.version,
                        package.objects.len()
                    );
                    out.push(ResolvedDependency {
                        dependency: AppDependency {
                            app_id: candidate.meta.app_id.clone(),
                            name: candidate.meta.name.clone(),
                            publisher: candidate.meta.publisher.clone(),
                            version: candidate.meta.version.clone(),
                        },
                        app_path: candidate.app_path.clone(),
                        package,
                        stamp: candidate.stamp,
                    });
                    winner_idx = Some(i);
                    break;
                }
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("{e:#}"));
                    let has_fallback = i + 1 < group.len();
                    warn!(
                        "load_all_apps: failed to parse {} v{}: {:#}{}",
                        candidate.app_path.display(),
                        candidate.meta.version,
                        e,
                        if has_fallback {
                            " — falling back to next-highest copy of this GUID"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        // No copy of this GUID could be read: record the best one, so the
        // dependency is reported unreadable rather than simply absent (S5.2).
        if winner_idx.is_none()
            && let (Some(best), Some(error)) = (group.first(), first_error)
        {
            unreadable_apps.push(UnreadableDependency {
                path: best.app_path.clone(),
                guid: Some(best.meta.app_id.clone()),
                name: Some(best.meta.name.clone()),
                version: Some(best.meta.version.clone()),
                error,
            });
        }
        // Every candidate ranked BELOW the winner (if one was found) is a
        // genuine dedup drop against the ACTUAL kept version — not
        // necessarily the group's nominal best, if a fallback occurred.
        if let Some(idx) = winner_idx {
            let winner = &group[idx];
            for loser in group.iter().skip(idx + 1) {
                dropped.push(DroppedDuplicateDependency {
                    guid: winner.meta.app_id.clone(),
                    name: winner.meta.name.clone(),
                    kept_version: winner.meta.version.clone(),
                    kept_path: winner.app_path.clone(),
                    dropped_version: loser.meta.version.clone(),
                    dropped_path: loser.app_path.clone(),
                });
            }
        }
    }

    // Deterministic, filesystem-independent order so downstream AppRef/NodeId
    // numbering is reproducible across machines (charter C8). `parse_version`
    // gives a REAL numeric multi-part comparison (not raw string) for the
    // version component — H-2 fix: the prior string `.cmp()` here was NOT
    // "purely a stable tiebreak" as its old comment claimed, since (pre-dedup)
    // downstream GUID-matching picked the first entry in this very order,
    // making this sort the de facto (and wrong — lexicographic, not semver)
    // version-selection policy. Post-dedup there is at most one entry per
    // non-empty GUID, so this now IS purely a determinism tiebreak — but it's
    // still fixed to a real comparator rather than left honest-but-wrong.
    out.sort_by(|a, b| {
        (
            &a.dependency.app_id,
            &a.dependency.name,
            &a.dependency.publisher,
            parse_version(&a.dependency.version),
        )
            .cmp(&(
                &b.dependency.app_id,
                &b.dependency.name,
                &b.dependency.publisher,
                parse_version(&b.dependency.version),
            ))
    });

    Ok((
        out,
        DependencyLoadReport {
            dropped,
            unreadable: unreadable_apps,
        },
    ))
}

/// The warning `load_all_apps` logs when a `.app`'s manifest cannot be read.
/// `{:#}` prints the whole context chain: plain `{}` printed only the outer
/// context ("Ready-to-Run package: <path>") and hid the real cause.
fn manifest_read_warning(path: &Path, e: &anyhow::Error) -> String {
    format!(
        "load_all_apps: failed to read manifest of {}: {e:#}",
        path.display()
    )
}

/// Resolve all dependencies for a project
///
/// Returns a list of resolved dependencies with their parsed packages.
/// Dependencies that cannot be resolved are logged as warnings and skipped.
pub fn resolve_all(project_root: &Path) -> Result<Vec<ResolvedDependency>> {
    let app_json_path = project_root.join("app.json");
    if !app_json_path.exists() {
        debug!("No app.json found at {}", project_root.display());
        return Ok(Vec::new());
    }

    let dependencies = parse_app_json(&app_json_path)?;
    if dependencies.is_empty() {
        debug!("No dependencies declared in app.json");
        return Ok(Vec::new());
    }

    let alpackages = match find_alpackages_folder(project_root) {
        Some(path) => path,
        None => {
            warn!("No .alpackages folder found at {}", project_root.display());
            return Ok(Vec::new());
        }
    };

    info!(
        "Resolving {} dependencies from {}",
        dependencies.len(),
        alpackages.display()
    );

    let mut resolved = Vec::new();

    for dep in dependencies {
        match find_matching_app(&alpackages, &dep) {
            Some(app_path) => {
                debug!(
                    "Found {} {} -> {}",
                    dep.name,
                    dep.version,
                    app_path.display()
                );

                let stamp = AppFileStamp::of(&app_path);
                match extract_app_package(&app_path) {
                    Ok(package) => {
                        info!(
                            "Loaded {} v{} ({} objects)",
                            package.metadata.name,
                            package.metadata.version,
                            package.objects.len()
                        );
                        resolved.push(ResolvedDependency {
                            dependency: dep,
                            app_path,
                            package: std::sync::Arc::new(package),
                            stamp,
                        });
                    }
                    Err(e) => {
                        warn!("Failed to parse {}: {:#}", app_path.display(), e);
                        #[cfg(feature = "telemetry")]
                        crate::telemetry::record_indexer_issue(
                            crate::telemetry::IndexerIssueKind::AppParseFailed,
                            0,
                            None,
                        );
                    }
                }
            }
            None => {
                warn!(
                    "Could not find matching .app for {} {} (publisher: {})",
                    dep.name, dep.version, dep.publisher
                );
                #[cfg(feature = "telemetry")]
                {
                    let dep_id = format!("{}:{}", dep.publisher, dep.name);
                    crate::telemetry::record_indexer_issue(
                        crate::telemetry::IndexerIssueKind::MissingDependency,
                        0,
                        Some(&dep_id),
                    );
                }
            }
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest-read warning must name the real cause, not just the
    /// outer context. A Ready-to-Run package naming a nested app it lacks
    /// fails with "Ready-to-Run package: <path>" outside and "missing.app"
    /// inside; plain `{}` printed only the outside.
    #[test]
    fn manifest_read_warning_includes_the_inner_cause() {
        let package = crate::engine::deps::app_package_zip::test_apps::build_app(&[(
            "readytorunappmanifest.json",
            br#"{"EmbeddedAppFileName":"missing.app"}"#,
        )]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Microsoft_Base Application_28.4.app");
        std::fs::write(&path, package).unwrap();

        let err = crate::app_package::extract_app_metadata(&path).unwrap_err();
        let line = manifest_read_warning(&path, &err);
        assert!(line.contains("Ready-to-Run package"), "{line}");
        assert!(line.contains("missing.app"), "{line}");
    }

    // -----------------------------------------------------------------------
    // beyond-1B.3b Task 5.5: append_implicit_ms_tier_deps
    // -----------------------------------------------------------------------

    #[test]
    fn implicit_deps_appended_when_application_and_platform_non_empty() {
        let mut declared: Vec<AppDependency> = Vec::new();
        append_implicit_ms_tier_deps(
            &mut declared,
            "aaaa0000-0000-0000-0000-000000000000",
            Some("24.0.0.0"),
            Some("24.0.0.0"),
        );
        // 3 MS_APPLICATION_TIER rows + 2 MS_PLATFORM_TIER rows.
        assert_eq!(declared.len(), 5, "got {declared:?}");
        assert!(
            declared
                .iter()
                .any(|d| d.app_id == "437dbf0e-84ff-417a-965d-ed2bb9650972"
                    && d.name == "Base Application"
                    && d.version == "24.0.0.0"),
            "Base App must be injected from `application`; got {declared:?}"
        );
        assert!(
            declared
                .iter()
                .any(|d| d.app_id == "63ca2fa4-4f03-4f2b-a480-172fef340d3f"
                    && d.name == "System Application"
                    && d.version == "24.0.0.0"),
            "System App must be injected from `platform`; got {declared:?}"
        );
    }

    #[test]
    fn implicit_deps_none_when_application_and_platform_absent() {
        let mut declared: Vec<AppDependency> = Vec::new();
        append_implicit_ms_tier_deps(
            &mut declared,
            "aaaa0000-0000-0000-0000-000000000000",
            None,
            None,
        );
        assert!(
            declared.is_empty(),
            "no application/platform must inject nothing; got {declared:?}"
        );
    }

    #[test]
    fn implicit_deps_none_when_application_and_platform_empty_string() {
        let mut declared: Vec<AppDependency> = Vec::new();
        append_implicit_ms_tier_deps(
            &mut declared,
            "aaaa0000-0000-0000-0000-000000000000",
            Some(""),
            Some(""),
        );
        assert!(
            declared.is_empty(),
            "empty-string application/platform must inject nothing (not just absent); got {declared:?}"
        );
    }

    #[test]
    fn implicit_deps_application_only_does_not_inject_platform_tier() {
        let mut declared: Vec<AppDependency> = Vec::new();
        append_implicit_ms_tier_deps(
            &mut declared,
            "aaaa0000-0000-0000-0000-000000000000",
            Some("24.0.0.0"),
            None,
        );
        assert_eq!(
            declared.len(),
            3,
            "only MS_APPLICATION_TIER; got {declared:?}"
        );
        assert!(
            declared
                .iter()
                .all(|d| d.app_id != "63ca2fa4-4f03-4f2b-a480-172fef340d3f"),
            "System App must NOT be injected when `platform` is absent; got {declared:?}"
        );
    }

    #[test]
    fn implicit_deps_self_guard_skips_own_guid() {
        // Base App's own GUID happens to equal an MS_APPLICATION_TIER entry —
        // it must never gain itself as an implicit dependency.
        let mut declared: Vec<AppDependency> = Vec::new();
        append_implicit_ms_tier_deps(
            &mut declared,
            "437dbf0e-84ff-417a-965d-ed2bb9650972", // Base App's own guid
            Some("24.0.0.0"),
            None,
        );
        assert_eq!(
            declared.len(),
            2,
            "Base App entry self-skipped; got {declared:?}"
        );
        assert!(
            declared
                .iter()
                .all(|d| d.app_id != "437dbf0e-84ff-417a-965d-ed2bb9650972"),
            "must not inject Base App as its own dependency; got {declared:?}"
        );
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("26.0.0.0"), vec![26, 0, 0, 0]);
        assert_eq!(parse_version("26.0.30643.32100"), vec![26, 0, 30643, 32100]);
        assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
    }

    #[test]
    fn test_is_version_compatible() {
        // Same major.minor
        assert!(is_version_compatible("26.0.0.0", "26.0.0.0"));
        assert!(is_version_compatible("26.0.0.0", "26.0.30643.32100"));

        // Higher minor
        assert!(is_version_compatible("26.0.0.0", "26.1.0.0"));

        // Lower minor - not compatible
        assert!(!is_version_compatible("26.1.0.0", "26.0.0.0"));

        // Higher major
        assert!(is_version_compatible("26.0.0.0", "27.0.0.0"));

        // Lower major - not compatible
        assert!(!is_version_compatible("27.0.0.0", "26.0.0.0"));
    }

    #[test]
    fn test_compare_versions() {
        use std::cmp::Ordering;

        // Higher version should come first (Less means a > b)
        assert_eq!(
            compare_versions("26.0.30643.32100", "26.0.30643.31340"),
            Ordering::Less
        );
        assert_eq!(compare_versions("26.0.0.0", "25.0.0.0"), Ordering::Less);
        assert_eq!(compare_versions("26.0.0.0", "26.0.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("25.0.0.0", "26.0.0.0"), Ordering::Greater);
    }

    // -----------------------------------------------------------------------
    // H-2 (Tier-1 remediation, Task T1.2): dedup_by_guid_keep_highest_version
    // -----------------------------------------------------------------------

    fn discovered(guid: &str, name: &str, version: &str, path: &str) -> DiscoveredApp {
        DiscoveredApp {
            stamp: None,
            app_path: PathBuf::from(path),
            meta: crate::app_package::AppMetadata {
                app_id: guid.to_string(),
                name: name.to_string(),
                publisher: "Pub".to_string(),
                version: version.to_string(),
                runtime: String::new(),
                platform: String::new(),
                application: String::new(),
                dependencies: vec![],
                internals_visible_to: vec![],
            },
        }
    }

    /// The "stale wins" scenario: two entries share a GUID at different
    /// versions, ordinary ascending-numeric order (24.0.0.0 < 25.0.0.0, no
    /// digit-count trickery needed — the OLD bug was simply "sort ascending,
    /// take first", which always favors the LOWEST version, not a semver
    /// edge case). The highest-version entry must survive, and the drop must
    /// be named.
    #[test]
    fn dedup_keeps_highest_version_and_names_the_dropped_file() {
        let deps = vec![
            discovered(
                "11111111-0000-0000-0000-000000000001",
                "DupApp",
                "24.0.0.0",
                "/alpackages/Pub_DupApp_24.0.0.0.app",
            ),
            discovered(
                "11111111-0000-0000-0000-000000000001",
                "DupApp",
                "25.0.0.0",
                "/alpackages/Pub_DupApp_25.0.0.0.app",
            ),
        ];

        let (kept, dropped) = dedup_by_guid_keep_highest_version(deps);

        assert_eq!(kept.len(), 1, "only the higher version must survive");
        assert_eq!(kept[0].meta.version, "25.0.0.0");

        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].guid, "11111111-0000-0000-0000-000000000001");
        assert_eq!(dropped[0].kept_version, "25.0.0.0");
        assert_eq!(dropped[0].dropped_version, "24.0.0.0");
        assert_eq!(
            dropped[0].dropped_path,
            PathBuf::from("/alpackages/Pub_DupApp_24.0.0.0.app")
        );
    }

    /// The genuinely-diverging-order case named in the task brief: a naive
    /// STRING sort of "10.0.0.0" vs "9.0.0.0" disagrees with numeric
    /// magnitude ("10.0.0.0" < "9.0.0.0" lexicographically). The real
    /// `compare_versions` comparator (multi-part numeric, already used
    /// elsewhere in this file) must still pick 10.0.0.0 as higher.
    #[test]
    fn dedup_picks_numerically_higher_version_even_when_lexicographically_smaller() {
        let deps = vec![
            discovered(
                "22222222-0000-0000-0000-000000000002",
                "DigitApp",
                "9.0.0.0",
                "/alpackages/Pub_DigitApp_9.0.0.0.app",
            ),
            discovered(
                "22222222-0000-0000-0000-000000000002",
                "DigitApp",
                "10.0.0.0",
                "/alpackages/Pub_DigitApp_10.0.0.0.app",
            ),
        ];

        let (kept, dropped) = dedup_by_guid_keep_highest_version(deps);

        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].meta.version, "10.0.0.0",
            "10.0.0.0 is numerically higher despite sorting lexicographically \
             smaller than 9.0.0.0"
        );
        assert_eq!(dropped[0].dropped_version, "9.0.0.0");
    }

    /// Byte-identical duplicate: same GUID, SAME version, present twice
    /// (e.g. the identical file physically copied into two scanned
    /// `.alpackages` folders). Must dedup cleanly to exactly one survivor —
    /// this is the input-side half of the H-2 fix that prevents
    /// `abi_overload_collapsed` poisoning downstream (see
    /// `program::build::dedup_routines_preserving_genuine_overloads`'s
    /// `abi_sig_fp_collision_marks_survivor_collapsed`, which proves what
    /// happens if a duplicate DOES reach ingestion — this test proves it
    /// never will).
    #[test]
    fn dedup_collapses_byte_identical_duplicate_pair_to_one_survivor() {
        let deps = vec![
            discovered(
                "33333333-0000-0000-0000-000000000003",
                "SameVerApp",
                "1.0.0.0",
                "/alpackages/a/Pub_SameVerApp_1.0.0.0.app",
            ),
            discovered(
                "33333333-0000-0000-0000-000000000003",
                "SameVerApp",
                "1.0.0.0",
                "/alpackages/b/Pub_SameVerApp_1.0.0.0.app",
            ),
        ];

        let (kept, dropped) = dedup_by_guid_keep_highest_version(deps);

        assert_eq!(
            kept.len(),
            1,
            "a byte-identical duplicate (same GUID, same version) must \
             collapse to exactly one survivor"
        );
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].kept_version, dropped[0].dropped_version);
    }

    /// GUID-less entries (a malformed/legacy manifest) are never deduped
    /// against each other — fail-closed: without a real identity, collapsing
    /// two coincidentally-similar entries risks silently merging two
    /// GENUINELY different apps.
    #[test]
    fn dedup_never_merges_guid_less_entries() {
        let deps = vec![
            discovered("", "NoGuidApp", "1.0.0.0", "/alpackages/a.app"),
            discovered("", "NoGuidApp", "1.0.0.0", "/alpackages/b.app"),
        ];

        let (kept, dropped) = dedup_by_guid_keep_highest_version(deps);

        assert_eq!(
            kept.len(),
            2,
            "GUID-less entries must never be collapsed against each other"
        );
        assert!(dropped.is_empty());
    }

    /// Control: a single entry per GUID (the ordinary, non-duplicated case)
    /// passes through untouched with no diagnostic.
    #[test]
    fn dedup_no_op_when_every_guid_is_unique() {
        let deps = vec![
            discovered(
                "44444444-0000-0000-0000-000000000004",
                "AppA",
                "1.0.0.0",
                "/alpackages/AppA.app",
            ),
            discovered(
                "55555555-0000-0000-0000-000000000005",
                "AppB",
                "2.0.0.0",
                "/alpackages/AppB.app",
            ),
        ];

        let (kept, dropped) = dedup_by_guid_keep_highest_version(deps);

        assert_eq!(kept.len(), 2);
        assert!(dropped.is_empty());
    }

    // -----------------------------------------------------------------------
    // Perf safe-wins Task 3: manifest-first dedup
    // -----------------------------------------------------------------------

    /// Like `snapshot::tests::write_minimal_app`, but with a caller-supplied
    /// SymbolReference payload so a test can plant a CORRUPT one.
    fn write_app_with_symbols(
        dir: &std::path::Path,
        filename: &str,
        guid: &str,
        version: &str,
        symbol_reference: &str,
    ) -> PathBuf {
        use std::io::Write;
        let manifest = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{guid}" Name="DupApp" Publisher="Pub" Version="{version}" Runtime="13.0" /></Package>"#
        );
        let mut zip_bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut zip_bytes);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            zip.start_file("NavxManifest.xml", options).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            zip.start_file("SymbolReference.json", options).unwrap();
            zip.write_all(symbol_reference.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let path = dir.join(filename);
        let mut out = std::fs::File::create(&path).unwrap();
        out.write_all(&[0u8; 40]).unwrap(); // NAVX header (content unused)
        out.write_all(zip_bytes.get_ref()).unwrap();
        path
    }

    /// Perf safe-wins Task 3: GUID dedup must happen on MANIFEST identity,
    /// BEFORE any SymbolReference.json is parsed. The stale 24.0 loser here
    /// carries deliberately corrupt symbols — under the old order (extract
    /// everything, then dedup) it fails extraction and is silently skipped
    /// (dropped list EMPTY); under manifest-first it must be reported as a
    /// proper dedup drop, and its symbol blob must never need to parse.
    #[test]
    fn guid_dedup_drops_loser_on_manifest_identity_without_parsing_its_symbols() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alpackages = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpackages).unwrap();
        let guid = "cccccccc-2222-2222-2222-222222222222";
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_24.0.0.0.app",
            guid,
            "24.0.0.0",
            "{ this is not JSON",
        );
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_25.0.0.0.app",
            guid,
            "25.0.0.0",
            r#"{"Codeunits":[{"Id":50100,"Name":"DupCU","Methods":[{"Name":"DoIt","Id":1}]}]}"#,
        );

        let (kept, report) = load_all_apps(dir.path()).expect("load_all_apps");
        let dropped = report.dropped;

        assert_eq!(kept.len(), 1, "exactly the 25.0 winner must survive");
        assert_eq!(kept[0].dependency.version, "25.0.0.0");
        assert_eq!(
            dropped.len(),
            1,
            "the 24.0 loser must be a REPORTED dedup drop — not a silent \
             extraction failure (which is what the old symbols-first order made it)"
        );
        assert_eq!(dropped[0].dropped_version, "24.0.0.0");
        assert_eq!(dropped[0].kept_version, "25.0.0.0");
    }

    /// Availability regression fix: when the manifest-first dedup WINNER's
    /// symbols are corrupt (fails `extract_app_symbols`), the dependency
    /// must not simply vanish — the fresh manifest-first ordering must fall
    /// back to the next-highest copy of the SAME GUID (here, a good 24.0.0.0
    /// sitting right alongside the corrupt 25.0.0.0) rather than dropping the
    /// dependency entirely (which the old symbols-first code never did: it
    /// tried every copy, in whatever order it found them, and only skipped
    /// the ones that actually failed to parse).
    #[test]
    fn corrupt_winner_falls_back_to_next_highest_good_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alpackages = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpackages).unwrap();
        let guid = "dddddddd-3333-3333-3333-333333333333";
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_25.0.0.0.app",
            guid,
            "25.0.0.0",
            "{ this is not JSON",
        );
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_24.0.0.0.app",
            guid,
            "24.0.0.0",
            r#"{"Codeunits":[{"Id":50100,"Name":"DupCU","Methods":[{"Name":"DoIt","Id":1}]}]}"#,
        );

        let (kept, report) = load_all_apps(dir.path()).expect("load_all_apps");
        let dropped = report.dropped;

        assert_eq!(
            kept.len(),
            1,
            "the good 24.0 copy must be loaded as a fallback, not dropped entirely"
        );
        assert_eq!(
            kept[0].dependency.version, "24.0.0.0",
            "the corrupt 25.0 winner must be skipped in favor of the good 24.0 copy"
        );
        assert!(
            !dropped
                .iter()
                .any(|d| d.dropped_version == "24.0.0.0" && d.guid == guid),
            "the promoted fallback copy (24.0) must NOT be reported as a dropped duplicate; got {dropped:#?}"
        );
    }

    /// If EVERY copy of a duplicated GUID fails symbol extraction, the
    /// dependency is legitimately absent — but this must never panic, and
    /// must not incorrectly report a "dropped duplicate" (there was no good
    /// survivor to keep).
    #[test]
    fn all_copies_corrupt_leaves_dependency_absent_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alpackages = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpackages).unwrap();
        let guid = "eeeeeeee-4444-4444-4444-444444444444";
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_25.0.0.0.app",
            guid,
            "25.0.0.0",
            "{ this is not JSON either",
        );
        write_app_with_symbols(
            &alpackages,
            "Pub_DupApp_24.0.0.0.app",
            guid,
            "24.0.0.0",
            "{ still not JSON",
        );

        let (kept, report) = load_all_apps(dir.path()).expect("load_all_apps");
        let dropped = report.dropped;

        assert!(
            !kept.iter().any(|k| k.dependency.app_id == guid),
            "no copy could be loaded, so the dependency must be absent"
        );
        assert!(
            !dropped.iter().any(|d| d.guid == guid),
            "with no surviving copy there is nothing to report as a dedup drop"
        );
    }

    #[test]
    fn test_resolve_real_project() {
        let test_path = Path::new("u:/Git/DO/Cloud");
        if !test_path.exists() {
            eprintln!("Skipping test: test project not found");
            return;
        }

        let result = resolve_all(test_path);
        assert!(result.is_ok(), "Failed to resolve: {:?}", result.err());

        let resolved = result.unwrap();
        println!("Resolved {} dependencies", resolved.len());

        for dep in &resolved {
            println!(
                "  {} v{} -> {} objects",
                dep.package.metadata.name,
                dep.package.metadata.version,
                dep.package.objects.len()
            );
        }

        // Should have at least the declared dependencies
        assert!(
            !resolved.is_empty(),
            "Should resolve at least one dependency"
        );
    }

    #[test]
    fn test_parse_app_json_real_project() {
        let app_json = Path::new("U:/Git/DO.Support-wi-75148/DocumentOutput/Cloud/app.json");
        if !app_json.exists() {
            eprintln!("Skipping test: DO.Support-wi-75148 not available");
            return;
        }

        let deps = parse_app_json(app_json).expect("Failed to parse app.json");
        assert!(!deps.is_empty(), "Should have dependencies");
        println!("Found {} dependencies", deps.len());
        for dep in &deps {
            println!("  {} by {} v{}", dep.name, dep.publisher, dep.version);
        }
    }

    #[test]
    fn test_find_alpackages_folder_exists() {
        let project = Path::new("U:/Git/DO.Support-wi-75148/DocumentOutput/Cloud");
        if !project.exists() {
            eprintln!("Skipping test: project not available");
            return;
        }

        let result = find_alpackages_folder(project);
        assert!(result.is_some(), "Should find .alpackages folder");
    }

    #[test]
    fn test_find_alpackages_folder_missing() {
        let result = find_alpackages_folder(Path::new("/nonexistent/path"));
        assert!(result.is_none());
    }

    #[test]
    fn test_find_matching_app_real() {
        let alpackages = Path::new("U:/Git/DO.Support-wi-75148/DocumentOutput/Cloud/.alpackages");
        if !alpackages.exists() {
            eprintln!("Skipping test: .alpackages not available");
            return;
        }

        let dep = AppDependency {
            app_id: String::new(),
            name: "Continia Core".to_string(),
            publisher: "Continia Software".to_string(),
            version: "29.0.0.0".to_string(),
        };

        let result = find_matching_app(alpackages, &dep);
        assert!(
            result.is_some(),
            "Should find matching .app for Continia Core"
        );
        let path = result.unwrap();
        assert!(
            path.to_string_lossy()
                .contains("Continia Software_Continia Core_")
        );
        println!("Found: {}", path.display());
    }

    #[test]
    fn test_find_matching_app_not_found() {
        let alpackages = Path::new("U:/Git/DO.Support-wi-75148/DocumentOutput/Cloud/.alpackages");
        if !alpackages.exists() {
            eprintln!("Skipping test: .alpackages not available");
            return;
        }

        let dep = AppDependency {
            app_id: String::new(),
            name: "NonExistent App".to_string(),
            publisher: "Nobody".to_string(),
            version: "1.0.0.0".to_string(),
        };

        let result = find_matching_app(alpackages, &dep);
        assert!(result.is_none(), "Should not find non-existent app");
    }

    #[test]
    fn test_resolve_all_do_support() {
        let project = Path::new("U:/Git/DO.Support-wi-75148/DocumentOutput/Cloud");
        if !project.exists() {
            eprintln!("Skipping test: DO.Support-wi-75148 not available");
            return;
        }

        let result = resolve_all(project);
        assert!(result.is_ok(), "Failed to resolve: {:?}", result.err());
        let resolved = result.unwrap();
        println!("Resolved {} dependencies", resolved.len());
        for dep in &resolved {
            println!(
                "  {} v{} -> {} objects",
                dep.package.metadata.name,
                dep.package.metadata.version,
                dep.package.objects.len()
            );
        }
        assert!(
            !resolved.is_empty(),
            "Should resolve at least one dependency"
        );
    }

    #[test]
    fn test_resolve_all_no_app_json() {
        // Use a temp dir with no app.json
        let dir = tempfile::TempDir::new().unwrap();
        let result = resolve_all(dir.path()).unwrap();
        assert!(result.is_empty(), "No app.json should return empty");
    }

    #[test]
    fn test_resolve_all_empty_dependencies() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("app.json"), r#"{"dependencies": []}"#).unwrap();
        let result = resolve_all(dir.path()).unwrap();
        assert!(result.is_empty(), "Empty dependencies should return empty");
    }

    #[test]
    fn test_resolve_all_no_alpackages() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"dependencies": [{"name": "Test", "publisher": "Pub", "version": "1.0.0.0"}]}"#,
        )
        .unwrap();
        let result = resolve_all(dir.path()).unwrap();
        assert!(result.is_empty(), "No .alpackages should return empty");
    }

    // ---------------------------------------------------------------
    // find_all_alpackages_folders — parallel coverage to the Go side's
    // DiscoverPackageCachePaths tests (al-language-server-go/wrapper/
    // project_packagecache_test.go). Keeping these in lockstep prevents
    // the Go and Rust ancestor-walk semantics from drifting apart.
    // ---------------------------------------------------------------

    #[test]
    fn find_all_alpackages_own_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".alpackages")).unwrap();

        let folders = find_all_alpackages_folders(&project);
        assert_eq!(folders.len(), 1, "expected single entry, got {:?}", folders);
        assert!(folders[0].ends_with(".alpackages"));
    }

    #[test]
    fn find_all_alpackages_finds_ancestor() {
        let dir = tempfile::TempDir::new().unwrap();
        let parent = dir.path().join("parent");
        let project = parent.join("Cloud");
        std::fs::create_dir_all(parent.join(".alpackages")).unwrap();
        std::fs::create_dir_all(project.join(".alpackages")).unwrap();

        let folders = find_all_alpackages_folders(&project);
        assert!(
            folders.len() >= 2,
            "expected own + ancestor, got {:?}",
            folders
        );
        assert!(folders[0].ends_with(project.join(".alpackages").as_path().file_name().unwrap()));
        assert!(
            folders
                .iter()
                .any(|f| f.starts_with(&parent) && !f.starts_with(&project)),
            "ancestor folder missing from {:?}",
            folders
        );
    }

    #[test]
    fn find_all_alpackages_stops_at_git_boundary() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let project = repo.join("Cloud");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(project.join(".alpackages")).unwrap();
        // .alpackages ABOVE the .git boundary should NOT be picked up.
        std::fs::create_dir_all(dir.path().join(".alpackages")).unwrap();

        let folders = find_all_alpackages_folders(&project);
        let above = dir.path().join(".alpackages");
        for f in &folders {
            assert!(
                std::fs::canonicalize(f).unwrap() != std::fs::canonicalize(&above).unwrap(),
                "walked past .git boundary: included {:?}",
                f
            );
        }
    }

    #[test]
    fn find_all_alpackages_no_dups() {
        let dir = tempfile::TempDir::new().unwrap();
        let parent = dir.path().join("parent");
        let project = parent.join("child");
        std::fs::create_dir_all(parent.join(".alpackages")).unwrap();
        std::fs::create_dir_all(project.join(".alpackages")).unwrap();

        let folders = find_all_alpackages_folders(&project);
        let mut canonical: Vec<_> = folders
            .iter()
            .map(|f| std::fs::canonicalize(f).unwrap())
            .collect();
        canonical.sort();
        let len_before = canonical.len();
        canonical.dedup();
        assert_eq!(
            canonical.len(),
            len_before,
            "duplicate path in {:?}",
            folders
        );
    }

    #[test]
    fn find_all_alpackages_depth_cap() {
        // Build a long ancestor chain with .alpackages in every level.
        // The walk should stop at the 8-deep cap regardless.
        let dir = tempfile::TempDir::new().unwrap();
        let mut current = dir.path().to_path_buf();
        // 12 levels of ancestor .alpackages
        for i in 0..12 {
            current = current.join(format!("level{}", i));
            std::fs::create_dir_all(current.join(".alpackages")).unwrap();
        }
        // Place project at the deepest level.
        let project = current.clone();

        let folders = find_all_alpackages_folders(&project);
        // Own + at most 8 ancestors = 9 total.
        assert!(
            folders.len() <= 9,
            "expected ≤ 9 (own + 8 ancestors), got {}",
            folders.len()
        );
    }
}
